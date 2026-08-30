//! Resolve a Wayland `app_id` to the desktop entry behind it.
//!
//! This is the one job the GNOME backend gets for free and the Niri backend
//! has to do itself. GNOME Shell runs a `WindowTracker` that already knows
//! which `Shell.App` owns a window, so the extension can hand LoFi a canonical
//! `.desktop` id, a display name, and an icon per window. Niri has no
//! equivalent — its IPC reports the raw `app_id` the client set (or, for
//! XWayland clients, the WM_CLASS) and nothing else — so the mapping has to
//! happen here.
//!
//! It matters for three things: the window rows' icons and app names, the
//! running-indicator dot on `Entry::Application` rows, and the focus-instead-
//! of-launch branch in `launch::activate`. All three degrade gracefully when
//! the mapping fails: an unresolved window still lists and still focuses, it
//! just shows no icon and its application row shows no dot.
//!
//! ## The lookup chain
//!
//! 1. `<app_id>.desktop` verbatim. This is the freedesktop convention and it
//!    covers the overwhelming majority of native Wayland clients — both
//!    reverse-DNS ids (`com.mitchellh.ghostty`) and short ones
//!    (`google-chrome`).
//! 2. `<app_id>.desktop` lowercased. Some toolkits capitalise the id
//!    (`Alacritty`, `Emacs`) where the desktop file is lowercase.
//! 3. A `StartupWMClass` match. This is the freedesktop-blessed escape hatch
//!    for exactly this problem, and it is what covers XWayland clients whose
//!    WM_CLASS bears no resemblance to their desktop file id.
//!
//! Step 3 needs an index over every installed desktop entry, so it is built
//! lazily and only once: steps 1 and 2 answer almost every window, and paying
//! for a second full scan of the desktop-file tree on every launch — when
//! `apps::gather_applications` has already done one — would be a real cost
//! against LoFi's "launches instantly" goal for a case that rarely fires.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;

use gio_unix::DesktopAppInfo;
use gtk::gio::prelude::*;

use crate::apps::application_directories;

/// What LoFi needs to know about the application behind a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedApp {
    /// Canonical `.desktop`-suffixed id. This is the join key against
    /// `Application::desktop_id`, so it must be canonical for the
    /// running-indicator and focus-vs-launch logic in `main` to line up.
    pub desktop_id: String,
    /// Display name from the desktop entry.
    pub name: Option<String>,
    /// Icon *identifier* (a theme name or an absolute path), not bytes —
    /// rendering is deferred to the GTK image widget, same as `apps.rs`.
    pub icon: Option<String>,
}

/// Caches the expensive half of the lookup chain across the windows of one
/// gather pass.
#[derive(Default)]
pub struct AppIdResolver {
    /// `None` until the first step-1/2 miss forces the scan; `Some` (possibly
    /// empty) after. Maps lowercased `StartupWMClass` to canonical desktop id.
    wm_class_index: RefCell<Option<HashMap<String, String>>>,
}

impl AppIdResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve `app_id`, or `None` if no desktop entry matches.
    pub fn resolve(&self, app_id: &str) -> Option<ResolvedApp> {
        if app_id.is_empty() {
            return None;
        }

        if let Some(info) = DesktopAppInfo::new(&format!("{app_id}.desktop")) {
            return Some(describe(&info));
        }

        let lowered = app_id.to_lowercase();
        if lowered != app_id
            && let Some(info) = DesktopAppInfo::new(&format!("{lowered}.desktop"))
        {
            return Some(describe(&info));
        }

        let desktop_id = {
            let mut index = self.wm_class_index.borrow_mut();
            let index = index.get_or_insert_with(build_wm_class_index);
            index.get(&lowered).cloned()
        }?;

        DesktopAppInfo::new(&desktop_id).map(|info| describe(&info))
    }
}

/// Project a `DesktopAppInfo` into the three fields LoFi reads.
///
/// `info.id()` is preferred over the filename stem for the same reason
/// `apps::gather_applications` prefers it: it is the canonical id GIO computed
/// (including any subdirectory prefix), and it is the key
/// `EntryRef::Application` and the MRU store round-trip on. The `desktop_id`
/// fallback is only hit for an info built from a path GIO doesn't consider
/// part of the search tree, which `DesktopAppInfo::new` can't produce.
fn describe(info: &DesktopAppInfo) -> ResolvedApp {
    let desktop_id = info.id().map(|id| id.to_string()).unwrap_or_else(|| {
        info.filename()
            .and_then(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_owned))
            .unwrap_or_default()
    });

    let name = Some(info.name().to_string()).filter(|s| !s.trim().is_empty());

    let icon = info
        .icon()
        .and_then(|i| IconExt::to_string(&i))
        .map(|gs| gs.to_string())
        .filter(|s| !s.trim().is_empty());

    ResolvedApp {
        desktop_id,
        name,
        icon,
    }
}

/// Scan every XDG application directory and index the entries that declare a
/// `StartupWMClass`, keyed by that class lowercased.
///
/// Walks the directories directly (rather than asking GIO for every installed
/// `AppInfo`) so it uses the same directory list, the same non-recursive walk,
/// and the same first-directory-wins shadowing as
/// `apps::gather_applications` — a window resolved here must name a desktop id
/// that the application list actually contains, or the running-indicator join
/// in `main` would silently miss.
///
/// Entries that fail `should_show()` are skipped for the same reason: they
/// aren't in the application list either.
fn build_wm_class_index() -> HashMap<String, String> {
    let mut index: HashMap<String, String> = HashMap::new();

    for dir in application_directories() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_desktop = path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(".desktop"));
            if !is_desktop {
                continue;
            }

            let Some(info) = DesktopAppInfo::from_filename(&path) else {
                continue;
            };
            if !info.should_show() {
                continue;
            }
            let Some(wm_class) = info.startup_wm_class() else {
                continue;
            };
            let key = wm_class.to_lowercase();
            if key.is_empty() {
                continue;
            }

            let desktop_id = describe(&info).desktop_id;
            if desktop_id.is_empty() {
                continue;
            }
            // First directory wins, matching XDG shadowing precedence and
            // `gather_applications`' dedup.
            index.entry(key).or_insert(desktop_id);
        }
    }

    index
}
