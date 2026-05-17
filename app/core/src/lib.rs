pub mod commands;
pub mod matcher;
pub mod mru;
pub use commands::compute_geometry;
pub use matcher::search;
pub use mru::{MruError, MruStore};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub name: String,
    pub desktop_id: String,
    pub icon: Option<String>,
    /// Runtime-only state: when `Some(id)`, the application has at least one
    /// open window and `id` is the most recently focused. Set by the platform
    /// layer (`lofi-gnome::main`) after gathering windows from the extension;
    /// not persisted, not part of `EntryRef`. `is_running` is equivalent to
    /// `recent_window_id.is_some()`.
    pub recent_window_id: Option<u64>,
}

/// An open window surfaced by the GNOME Shell extension over D-Bus. `app_name`
/// and `icon` come from `Shell.WindowTracker`, which can return null for system
/// windows; both are `Option<String>` and the extension coerces empty strings
/// to `None` on the Rust side (see `app/gnome/src/windows.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub app_name: Option<String>,
    pub icon: Option<String>,
    pub workspace: i32,
    /// Canonical `.desktop`-suffixed id of the application backing this
    /// window, as resolved by `Shell.WindowTracker.get_window_app(...).get_id()`
    /// in the extension. `None` when the extension reported an empty string
    /// (no Shell.App for this window — system surfaces, override-redirect
    /// children). Used by the combine step in `lofi-gnome::main` to build the
    /// MRU map keyed on the matching `Application.desktop_id`.
    pub app_desktop_id: Option<String>,
}

/// A GNOME workspace surfaced by the Shell extension over D-Bus. `index` is
/// the 0-based workspace index used by Mutter; `name` is the human-readable
/// label (the extension currently hardcodes `"Workspace N"`, but a custom
/// naming extension would flow its label through here verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub index: i32,
    pub name: String,
}

/// Identifier for the launcher's static window-action commands. Each variant
/// maps to a stable snake_case id (`CommandKind::as_id`) that round-trips into
/// `EntryRef::Command(String)` so the persistent MRU store stays valid across
/// sessions even if the runtime command list is regenerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Center,
    CenterHalf,
    CenterTwoThirds,
    LeftHalf,
    RightHalf,
    StandardSize,
    Minimize,
    ToggleMaximize,
    ToggleFullscreen,
}

impl CommandKind {
    /// Stable snake_case identifier for this kind. Used as the payload of
    /// `EntryRef::Command(String)` (and therefore the persistent MRU key), so
    /// it must remain backwards-compatible across releases — adding a variant
    /// is fine, renaming an existing one would invalidate stored history.
    pub fn as_id(&self) -> &'static str {
        match self {
            CommandKind::Center => "center",
            CommandKind::CenterHalf => "center_half",
            CommandKind::CenterTwoThirds => "center_two_thirds",
            CommandKind::LeftHalf => "left_half",
            CommandKind::RightHalf => "right_half",
            CommandKind::StandardSize => "standard_size",
            CommandKind::Minimize => "minimize",
            CommandKind::ToggleMaximize => "toggle_maximize",
            CommandKind::ToggleFullscreen => "toggle_fullscreen",
        }
    }

    /// Human-readable label shown in the launcher list and used as the
    /// matcher haystack. Singular and lowercase-after-the-first-word so it
    /// reads naturally next to the existing entries.
    pub fn display_name(&self) -> &'static str {
        match self {
            CommandKind::Center => "Center",
            CommandKind::CenterHalf => "Center half",
            CommandKind::CenterTwoThirds => "Center two-thirds",
            CommandKind::LeftHalf => "Left half",
            CommandKind::RightHalf => "Right half",
            CommandKind::StandardSize => "Standard size",
            CommandKind::Minimize => "Minimize",
            CommandKind::ToggleMaximize => "Toggle maximize",
            CommandKind::ToggleFullscreen => "Toggle fullscreen",
        }
    }

    /// Symbolic icon name (Adwaita / freedesktop-symbolic) shown beside the
    /// command in the launcher. Picked to communicate the geometry shape
    /// (`view-dual-symbolic` for halves) or the action (`window-minimize-…`).
    pub fn icon_name(&self) -> &'static str {
        match self {
            CommandKind::Center => "focus-windows-symbolic",
            CommandKind::CenterHalf => "view-dual-symbolic",
            CommandKind::CenterTwoThirds => "sidebar-show-symbolic",
            CommandKind::LeftHalf => "view-dual-symbolic",
            CommandKind::RightHalf => "view-dual-symbolic",
            CommandKind::StandardSize => "focus-windows-symbolic",
            CommandKind::Minimize => "window-minimize-symbolic",
            CommandKind::ToggleMaximize => "window-maximize-symbolic",
            CommandKind::ToggleFullscreen => "view-fullscreen-symbolic",
        }
    }

    /// Inverse of `as_id`: parse a snake_case id back to a `CommandKind`.
    /// Used at MRU-rehydrate time when we re-materialize stored
    /// `EntryRef::Command(id)` entries. Returns `None` for unknown ids so
    /// stale rows in MRU silently fall off rather than panic.
    pub fn from_id(id: &str) -> Option<CommandKind> {
        match id {
            "center" => Some(CommandKind::Center),
            "center_half" => Some(CommandKind::CenterHalf),
            "center_two_thirds" => Some(CommandKind::CenterTwoThirds),
            "left_half" => Some(CommandKind::LeftHalf),
            "right_half" => Some(CommandKind::RightHalf),
            "standard_size" => Some(CommandKind::StandardSize),
            "minimize" => Some(CommandKind::Minimize),
            "toggle_maximize" => Some(CommandKind::ToggleMaximize),
            "toggle_fullscreen" => Some(CommandKind::ToggleFullscreen),
            _ => None,
        }
    }
}

/// Identifier for the launcher's system-level power commands. Each variant
/// maps to a stable snake_case id (`PowerCommandKind::as_id`) that round-trips
/// into `EntryRef::PowerCommand(String)` so the persistent MRU store stays
/// valid across sessions even if the runtime list is regenerated. Unlike the
/// window-action `CommandKind`, these commands always apply regardless of
/// focused window — they only need the kind, not a target/work area/frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerCommandKind {
    LockSession,
    Logout,
    Suspend,
    Restart,
    Shutdown,
}

impl PowerCommandKind {
    /// Stable snake_case identifier for this kind. Used as the payload of
    /// `EntryRef::PowerCommand(String)` (and therefore the persistent MRU
    /// key), so it must remain backwards-compatible across releases — adding
    /// a variant is fine, renaming an existing one would invalidate stored
    /// history.
    pub fn as_id(&self) -> &'static str {
        match self {
            PowerCommandKind::LockSession => "lock_session",
            PowerCommandKind::Logout => "logout",
            PowerCommandKind::Suspend => "suspend",
            PowerCommandKind::Restart => "restart",
            PowerCommandKind::Shutdown => "shutdown",
        }
    }

    /// Human-readable label shown in the launcher list and used as the
    /// matcher haystack. Short verb-like labels mirror what the GNOME
    /// system menu uses.
    pub fn display_name(&self) -> &'static str {
        match self {
            PowerCommandKind::LockSession => "Lock",
            PowerCommandKind::Logout => "Log Out",
            PowerCommandKind::Suspend => "Suspend",
            PowerCommandKind::Restart => "Restart",
            PowerCommandKind::Shutdown => "Shutdown",
        }
    }

    /// Symbolic icon name (Adwaita / freedesktop-symbolic) shown beside the
    /// command in the launcher. Picked to convey the action: a padlock for
    /// Lock, a door/arrow for Log Out, a moon for Suspend, a reboot arrow
    /// for Restart, a power symbol for Shutdown.
    pub fn icon_name(&self) -> &'static str {
        match self {
            PowerCommandKind::LockSession => "system-lock-screen-symbolic",
            PowerCommandKind::Logout => "system-log-out-symbolic",
            PowerCommandKind::Suspend => "weather-clear-night-symbolic",
            PowerCommandKind::Restart => "system-reboot-symbolic",
            PowerCommandKind::Shutdown => "system-shutdown-symbolic",
        }
    }

    /// Inverse of `as_id`: parse a snake_case id back to a `PowerCommandKind`.
    /// Used at MRU-rehydrate time when we re-materialize stored
    /// `EntryRef::PowerCommand(id)` entries. Returns `None` for unknown ids
    /// so stale rows in MRU silently fall off rather than panic.
    pub fn from_id(id: &str) -> Option<PowerCommandKind> {
        match id {
            "lock_session" => Some(PowerCommandKind::LockSession),
            "logout" => Some(PowerCommandKind::Logout),
            "suspend" => Some(PowerCommandKind::Suspend),
            "restart" => Some(PowerCommandKind::Restart),
            "shutdown" => Some(PowerCommandKind::Shutdown),
            _ => None,
        }
    }
}

/// A launcher entry representing a system-level power command (Lock, Suspend,
/// Restart, Shutdown). Distinct from `Command` because no per-window target,
/// work area, or current frame is required — the command always applies. The
/// wrapping struct (rather than a bare `PowerCommandKind` on `Entry`) keeps
/// the variant shape consistent with `Application`/`Window`/`Workspace`/`Command`
/// so future per-instance state can be added without a variant rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerCommand {
    pub kind: PowerCommandKind,
}

/// Mutter work area (the monitor rectangle minus panel/dock struts) for a
/// specific window's monitor. Used as the bounding box for every geometry
/// command. Captured at gather time from `GetWindowWorkArea(id)` so the
/// computed geometry stays correct even when LoFi itself is on a different
/// monitor than the target window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A launcher entry representing a window-action command (e.g. "Center half",
/// "Minimize"). Every command targets the previously-focused user window
/// captured at gather time — see `app/gnome/src/commands.rs::gather_commands`
/// for the LoFi-filter rationale. `work_area` and `current_frame` are also
/// captured at gather time so activation is a single D-Bus round-trip with no
/// further reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub kind: CommandKind,
    pub target_window_id: u64,
    pub work_area: WorkArea,
    /// `(x, y, width, height)` of the target window's frame at gather time.
    /// Only `CommandKind::Center` actually reads this (it keeps the current
    /// size and recenters); other kinds ignore it.
    pub current_frame: (i32, i32, i32, i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Application,
    Window,
    Workspace,
    Command,
    PowerCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Application(Application),
    Window(Window),
    Workspace(Workspace),
    Command(Command),
    PowerCommand(PowerCommand),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum EntryRef {
    Application(String),
    Window(u64),
    Workspace(i32),
    Command(String),
    PowerCommand(String),
}

/// Icon name used for every `Entry::Workspace`. Hardcoded because workspaces
/// don't have per-instance icons — the extension doesn't emit one and there's
/// nothing useful to vary on (index/name aren't visual). Kept as a `&'static
/// str` constant so `Entry::icon()` can borrow it without owning a String.
const WORKSPACE_ICON: &str = "view-grid-symbolic";

impl Entry {
    pub fn name(&self) -> &str {
        match self {
            Entry::Application(app) => app.name.as_str(),
            Entry::Window(w) => w.title.as_str(),
            Entry::Workspace(w) => w.name.as_str(),
            Entry::Command(c) => c.kind.display_name(),
            Entry::PowerCommand(c) => c.kind.display_name(),
        }
    }

    pub fn icon(&self) -> Option<&str> {
        match self {
            Entry::Application(app) => app.icon.as_deref(),
            Entry::Window(w) => w.icon.as_deref(),
            Entry::Workspace(_) => Some(WORKSPACE_ICON),
            Entry::Command(c) => Some(c.kind.icon_name()),
            Entry::PowerCommand(c) => Some(c.kind.icon_name()),
        }
    }

    pub fn kind(&self) -> EntryKind {
        match self {
            Entry::Application(_) => EntryKind::Application,
            Entry::Window(_) => EntryKind::Window,
            Entry::Workspace(_) => EntryKind::Workspace,
            Entry::Command(_) => EntryKind::Command,
            Entry::PowerCommand(_) => EntryKind::PowerCommand,
        }
    }

    pub fn reference(&self) -> EntryRef {
        match self {
            Entry::Application(app) => EntryRef::Application(app.desktop_id.clone()),
            Entry::Window(w) => EntryRef::Window(w.id),
            Entry::Workspace(w) => EntryRef::Workspace(w.index),
            Entry::Command(c) => EntryRef::Command(c.kind.as_id().to_string()),
            Entry::PowerCommand(c) => EntryRef::PowerCommand(c.kind.as_id().to_string()),
        }
    }
}

pub fn resolve<'a>(entries: &'a [Entry], reference: &EntryRef) -> Option<&'a Entry> {
    entries.iter().find(|e| &e.reference() == reference)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_application(name: &str, desktop_id: &str, icon: Option<&str>) -> Application {
        Application {
            name: name.to_string(),
            desktop_id: desktop_id.to_string(),
            icon: icon.map(str::to_string),
            recent_window_id: None,
        }
    }

    fn make_application_running(
        name: &str,
        desktop_id: &str,
        icon: Option<&str>,
        window_id: u64,
    ) -> Application {
        Application {
            name: name.to_string(),
            desktop_id: desktop_id.to_string(),
            icon: icon.map(str::to_string),
            recent_window_id: Some(window_id),
        }
    }

    fn make_window(id: u64, title: &str, app_name: Option<&str>, icon: Option<&str>) -> Window {
        Window {
            id,
            title: title.to_string(),
            app_name: app_name.map(str::to_string),
            icon: icon.map(str::to_string),
            workspace: 0,
            app_desktop_id: None,
        }
    }

    fn make_workspace(index: i32, name: &str) -> Workspace {
        Workspace {
            index,
            name: name.to_string(),
        }
    }

    /// Test helper: build a `Command` with a fixed work area and current frame.
    /// Used by the Command-variant Entry tests below. The numbers are
    /// deliberately non-zero so any field that's silently dropped surfaces as a
    /// mismatch in round-trip / resolve assertions.
    fn make_command(kind: CommandKind) -> Command {
        Command {
            kind,
            target_window_id: 42,
            work_area: WorkArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            current_frame: (100, 100, 800, 600),
        }
    }

    /// Test helper: build a `PowerCommand` for the given kind. Mirrors
    /// `make_command` for the window-action commands. `PowerCommand` has no
    /// per-instance state beyond the kind, but a helper keeps the test
    /// fixtures aligned with the surrounding patterns.
    fn make_power_command(kind: PowerCommandKind) -> PowerCommand {
        PowerCommand { kind }
    }

    #[test]
    fn entry_reference_round_trips_application() {
        let app = make_application("Firefox", "firefox.desktop", Some("firefox"));
        let entry = Entry::Application(app.clone());

        let reference = entry.reference();
        assert_eq!(
            reference,
            EntryRef::Application(app.desktop_id.clone()),
            "entry.reference() should be EntryRef::Application(desktop_id); got {reference:?}"
        );

        let entries = vec![entry.clone()];
        let resolved = resolve(&entries, &entry.reference());
        assert!(
            matches!(resolved, Some(r) if r == &entry),
            "resolve should return Some(&entry) for its own reference; got {resolved:?}"
        );
    }

    #[test]
    fn resolve_finds_application_by_reference() {
        let entries = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
            Entry::Application(make_application("Gamma", "gamma.desktop", None)),
        ];

        let reference = EntryRef::Application("beta.desktop".into());
        let resolved = resolve(&entries, &reference);

        let found = resolved.expect("resolve should find an entry for beta.desktop");
        assert_eq!(
            found.name(),
            "Beta",
            "resolve should return the Beta entry, not the first; got {:?}",
            found.name()
        );
    }

    #[test]
    fn resolve_returns_none_for_missing_reference() {
        let entries = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
            Entry::Application(make_application("Gamma", "gamma.desktop", None)),
        ];

        let missing = EntryRef::Application("missing.desktop".into());
        assert_eq!(
            resolve(&entries, &missing),
            None,
            "resolve should return None for a desktop_id not in the slice"
        );

        let empty: [Entry; 0] = [];
        let anything = EntryRef::Application("anything.desktop".into());
        assert_eq!(
            resolve(&empty, &anything),
            None,
            "resolve over an empty slice should always return None"
        );
    }

    #[test]
    fn entry_ref_serializes_to_tagged_json() {
        let r = EntryRef::Application("firefox.desktop".into());

        let serialized = serde_json::to_string(&r).expect("EntryRef should serialize to JSON");
        assert_eq!(
            serialized, r#"{"type":"application","id":"firefox.desktop"}"#,
            "EntryRef should serialize with tag=type/content=id and snake_case variant; got {serialized}"
        );

        let round_tripped: EntryRef =
            serde_json::from_str(&serialized).expect("EntryRef should deserialize from JSON");
        assert_eq!(
            round_tripped, r,
            "EntryRef should round-trip via serde_json; got {round_tripped:?}"
        );
    }

    #[test]
    fn entry_methods_return_application_data() {
        let app = make_application("Firefox", "firefox.desktop", Some("firefox"));
        let entry = Entry::Application(app);

        assert_eq!(
            entry.name(),
            "Firefox",
            "Entry::name should return the app name"
        );
        assert_eq!(
            entry.icon(),
            Some("firefox"),
            "Entry::icon should return the app icon as a borrowed &str"
        );
        assert_eq!(
            entry.kind(),
            EntryKind::Application,
            "Entry::kind should return EntryKind::Application for an Application variant"
        );

        let no_icon = Entry::Application(make_application("Bare", "bare.desktop", None));
        assert_eq!(
            no_icon.icon(),
            None,
            "Entry::icon should return None when the underlying Application has no icon"
        );
    }

    #[test]
    fn entry_application_running_round_trips() {
        // An Application with a recent_window_id (i.e. "running") must behave
        // identically to a non-running Application for all Entry accessors:
        // name/icon/kind/reference. In particular, EntryRef::Application is
        // still keyed solely by desktop_id — the runtime-only recent_window_id
        // field must NOT affect the reference.
        const RECENT_WINDOW_ID: u64 = 42;

        let running = make_application_running(
            "Firefox",
            "firefox.desktop",
            Some("firefox"),
            RECENT_WINDOW_ID,
        );
        let stopped = make_application("Firefox", "firefox.desktop", Some("firefox"));

        assert_eq!(
            running.recent_window_id,
            Some(RECENT_WINDOW_ID),
            "make_application_running should set recent_window_id; got {:?}",
            running.recent_window_id
        );
        assert_eq!(
            stopped.recent_window_id, None,
            "make_application should default recent_window_id to None; got {:?}",
            stopped.recent_window_id
        );

        let running_entry = Entry::Application(running.clone());
        let stopped_entry = Entry::Application(stopped.clone());

        assert_eq!(
            running_entry.name(),
            stopped_entry.name(),
            "Entry::name should be unaffected by recent_window_id; running={:?} stopped={:?}",
            running_entry.name(),
            stopped_entry.name()
        );
        assert_eq!(
            running_entry.icon(),
            stopped_entry.icon(),
            "Entry::icon should be unaffected by recent_window_id; running={:?} stopped={:?}",
            running_entry.icon(),
            stopped_entry.icon()
        );
        assert_eq!(
            running_entry.kind(),
            stopped_entry.kind(),
            "Entry::kind should be unaffected by recent_window_id; running={:?} stopped={:?}",
            running_entry.kind(),
            stopped_entry.kind()
        );
        assert_eq!(
            running_entry.kind(),
            EntryKind::Application,
            "Entry::kind for a running Application should still be EntryKind::Application; got {:?}",
            running_entry.kind()
        );

        // The reference must still be EntryRef::Application(desktop_id), with
        // no influence from recent_window_id.
        let running_ref = running_entry.reference();
        let stopped_ref = stopped_entry.reference();
        assert_eq!(
            running_ref, stopped_ref,
            "EntryRef for a running Application must equal the stopped one (recent_window_id is not part of EntryRef); running={running_ref:?} stopped={stopped_ref:?}"
        );
        assert_eq!(
            running_ref,
            EntryRef::Application("firefox.desktop".into()),
            "EntryRef::Application should be keyed solely by desktop_id; got {running_ref:?}"
        );

        // resolve() must still find the running entry by its own reference.
        let entries = vec![running_entry.clone()];
        let resolved = resolve(&entries, &running_entry.reference());
        assert!(
            matches!(resolved, Some(r) if r == &running_entry),
            "resolve should return Some(&running_entry) for its own reference; got {resolved:?}"
        );
    }

    #[test]
    fn entry_window_reference_round_trips() {
        let entry = Entry::Window(make_window(
            42,
            "GitHub — Pull Requests",
            Some("Firefox"),
            None,
        ));

        let reference = entry.reference();
        assert_eq!(
            reference,
            EntryRef::Window(42),
            "Entry::Window::reference() should be EntryRef::Window(id); got {reference:?}"
        );

        let entries = vec![entry.clone()];
        let resolved = resolve(&entries, &entry.reference());
        assert!(
            matches!(resolved, Some(r) if r == &entry),
            "resolve should return Some(&entry) for its own Window reference; got {resolved:?}"
        );

        // id=0 is a legal Mutter window id; must round-trip as well.
        let zero_entry = Entry::Window(make_window(0, "Zero", None, None));
        let zero_entries = vec![zero_entry.clone()];
        let zero_resolved = resolve(&zero_entries, &zero_entry.reference());
        assert!(
            matches!(zero_resolved, Some(r) if r == &zero_entry),
            "EntryRef::Window(0) should round-trip via resolve; got {zero_resolved:?}"
        );
        assert_eq!(
            zero_entry.reference(),
            EntryRef::Window(0),
            "Entry::Window with id=0 should reference EntryRef::Window(0)"
        );
    }

    #[test]
    fn resolve_finds_window_by_reference() {
        let entries = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
            Entry::Window(make_window(100, "First Window", Some("Firefox"), None)),
            Entry::Window(make_window(200, "Second Window", Some("Firefox"), None)),
            Entry::Window(make_window(300, "Third Window", Some("Thunderbird"), None)),
        ];

        let resolved = resolve(&entries, &EntryRef::Window(200));
        let found = resolved.expect("resolve should find a Window for id 200");
        assert_eq!(
            found.name(),
            "Second Window",
            "resolve should return the window whose id is 200; got {:?}",
            found.name()
        );

        // Missing Application reference returns None.
        let missing_app = EntryRef::Application("missing.desktop".into());
        assert_eq!(
            resolve(&entries, &missing_app),
            None,
            "resolve should return None for an Application desktop_id not in the slice"
        );

        // Missing Window id returns None.
        let missing_window = EntryRef::Window(999);
        assert_eq!(
            resolve(&entries, &missing_window),
            None,
            "resolve should return None for a Window id not in the slice"
        );

        // A Window ref must never resolve to an Application; sanity-check
        // with a Vec that has only Applications.
        let only_apps = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
        ];
        assert_eq!(
            resolve(&only_apps, &EntryRef::Window(123)),
            None,
            "EntryRef::Window must never resolve to an Application entry"
        );
    }

    #[test]
    fn entry_ref_window_serializes_to_tagged_json() {
        let r = EntryRef::Window(12345);

        let serialized = serde_json::to_string(&r).expect("EntryRef::Window should serialize");
        assert_eq!(
            serialized, r#"{"type":"window","id":12345}"#,
            "EntryRef::Window should serialize with tag=type/content=id and snake_case variant; got {serialized}"
        );

        let round_tripped: EntryRef =
            serde_json::from_str(&serialized).expect("EntryRef::Window should deserialize");
        assert_eq!(
            round_tripped, r,
            "EntryRef::Window should round-trip via serde_json; got {round_tripped:?}"
        );
    }

    #[test]
    fn entry_window_methods_return_window_data() {
        let entry = Entry::Window(make_window(
            7,
            "Tab Title",
            Some("Firefox"),
            Some("firefox"),
        ));

        assert_eq!(
            entry.name(),
            "Tab Title",
            "Entry::Window::name should return the window title"
        );
        assert_eq!(
            entry.icon(),
            Some("firefox"),
            "Entry::Window::icon should return the window icon as a borrowed &str"
        );
        assert_eq!(
            entry.kind(),
            EntryKind::Window,
            "Entry::Window::kind should return EntryKind::Window"
        );

        let no_icon = Entry::Window(make_window(8, "No Icon", Some("Firefox"), None));
        assert_eq!(
            no_icon.icon(),
            None,
            "Entry::Window::icon should return None when the underlying Window has no icon"
        );
    }

    #[test]
    fn entry_workspace_reference_round_trips() {
        let entry = Entry::Workspace(make_workspace(2, "Workspace 3"));

        let reference = entry.reference();
        assert_eq!(
            reference,
            EntryRef::Workspace(2),
            "Entry::Workspace::reference() should be EntryRef::Workspace(index); got {reference:?}"
        );

        let entries = vec![entry.clone()];
        let resolved = resolve(&entries, &entry.reference());
        assert!(
            matches!(resolved, Some(r) if r == &entry),
            "resolve should return Some(&entry) for its own Workspace reference; got {resolved:?}"
        );

        // index=0 is a legal workspace index (GNOME's first workspace);
        // it must round-trip as well.
        let zero_entry = Entry::Workspace(make_workspace(0, "Workspace 1"));
        let zero_entries = vec![zero_entry.clone()];
        let zero_resolved = resolve(&zero_entries, &zero_entry.reference());
        assert!(
            matches!(zero_resolved, Some(r) if r == &zero_entry),
            "EntryRef::Workspace(0) should round-trip via resolve; got {zero_resolved:?}"
        );
        assert_eq!(
            zero_entry.reference(),
            EntryRef::Workspace(0),
            "Entry::Workspace with index=0 should reference EntryRef::Workspace(0)"
        );
    }

    #[test]
    fn resolve_finds_workspace_by_reference() {
        let entries = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Window(make_window(2, "Window Two", Some("Firefox"), None)),
            Entry::Workspace(make_workspace(0, "Workspace 1")),
            Entry::Workspace(make_workspace(1, "Workspace 2")),
            Entry::Workspace(make_workspace(2, "Workspace 3")),
        ];

        let resolved = resolve(&entries, &EntryRef::Workspace(2));
        let found = resolved.expect("resolve should find a Workspace for index 2");
        assert_eq!(
            found.name(),
            "Workspace 3",
            "resolve should return the workspace whose index is 2; got {:?}",
            found.name()
        );
        assert_eq!(
            found.kind(),
            EntryKind::Workspace,
            "resolve(&EntryRef::Workspace(2)) must return a Workspace entry; got kind {:?}",
            found.kind()
        );

        // Cross-variant: a Window(2) ref must NOT resolve to a Workspace(2).
        let by_window_2 = resolve(&entries, &EntryRef::Window(2))
            .expect("resolve should find the Window with id 2");
        assert_eq!(
            by_window_2.kind(),
            EntryKind::Window,
            "EntryRef::Window(2) must resolve to a Window, not a Workspace; got kind {:?}",
            by_window_2.kind()
        );
        assert_eq!(
            by_window_2.name(),
            "Window Two",
            "EntryRef::Window(2) must resolve to the Window named \"Window Two\"; got {:?}",
            by_window_2.name()
        );

        // Cross-variant (other direction): EntryRef::Workspace(2) must NOT
        // resolve to the Window with id 2.
        let by_workspace_2 = resolve(&entries, &EntryRef::Workspace(2))
            .expect("resolve should find the Workspace with index 2");
        assert_eq!(
            by_workspace_2.kind(),
            EntryKind::Workspace,
            "EntryRef::Workspace(2) must resolve to a Workspace, not a Window; got kind {:?}",
            by_workspace_2.kind()
        );

        // Missing workspace index returns None.
        let missing = EntryRef::Workspace(99);
        assert_eq!(
            resolve(&entries, &missing),
            None,
            "resolve should return None for a Workspace index not in the slice"
        );
    }

    #[test]
    fn entry_ref_workspace_serializes_to_tagged_json() {
        let r = EntryRef::Workspace(2);

        let serialized = serde_json::to_string(&r).expect("EntryRef::Workspace should serialize");
        assert_eq!(
            serialized, r#"{"type":"workspace","id":2}"#,
            "EntryRef::Workspace should serialize with tag=type/content=id and snake_case variant; got {serialized}"
        );

        let round_tripped: EntryRef =
            serde_json::from_str(&serialized).expect("EntryRef::Workspace should deserialize");
        assert_eq!(
            round_tripped, r,
            "EntryRef::Workspace should round-trip via serde_json; got {round_tripped:?}"
        );
    }

    #[test]
    fn entry_workspace_methods_return_workspace_data() {
        let entry = Entry::Workspace(make_workspace(1, "Editor"));

        assert_eq!(
            entry.name(),
            "Editor",
            "Entry::Workspace::name should return the workspace name"
        );
        assert_eq!(
            entry.icon(),
            Some("view-grid-symbolic"),
            "Entry::Workspace::icon should return the hardcoded \"view-grid-symbolic\" constant"
        );
        assert_eq!(
            entry.kind(),
            EntryKind::Workspace,
            "Entry::Workspace::kind should return EntryKind::Workspace"
        );
    }

    /// Exhaustive list of all `CommandKind` variants, kept in one place so the
    /// round-trip tests below stay synchronized. If a variant is added, this
    /// list must grow — and the tests will fail loudly until the maintainer
    /// extends it.
    const ALL_COMMAND_KINDS: &[CommandKind] = &[
        CommandKind::Center,
        CommandKind::CenterHalf,
        CommandKind::CenterTwoThirds,
        CommandKind::LeftHalf,
        CommandKind::RightHalf,
        CommandKind::StandardSize,
        CommandKind::Minimize,
        CommandKind::ToggleMaximize,
        CommandKind::ToggleFullscreen,
    ];

    #[test]
    fn entry_command_reference_round_trips() {
        for &kind in ALL_COMMAND_KINDS {
            let entry = Entry::Command(make_command(kind));
            let reference = entry.reference();
            let expected_ref = EntryRef::Command(kind.as_id().into());
            assert_eq!(
                reference, expected_ref,
                "Entry::Command({kind:?}).reference() should be EntryRef::Command(kind.as_id()); got {reference:?}, want {expected_ref:?}"
            );

            let entries = vec![entry.clone()];
            let resolved = resolve(&entries, &entry.reference());
            assert!(
                matches!(resolved, Some(r) if r == &entry),
                "resolve should return Some(&entry) for the Command reference of kind {kind:?}; got {resolved:?}"
            );
        }
    }

    #[test]
    fn resolve_finds_command_by_reference() {
        let entries = vec![
            Entry::Application(make_application("Center", "center.desktop", None)),
            Entry::Window(make_window(7, "A Window", Some("Firefox"), None)),
            Entry::Workspace(make_workspace(0, "Workspace 1")),
            Entry::Command(make_command(CommandKind::Center)),
            Entry::Command(make_command(CommandKind::CenterHalf)),
            Entry::Command(make_command(CommandKind::Minimize)),
        ];

        let reference = EntryRef::Command(CommandKind::CenterHalf.as_id().into());
        let resolved = resolve(&entries, &reference);
        let found = resolved.expect("resolve should find a Command for center_half");
        assert_eq!(
            found.kind(),
            EntryKind::Command,
            "resolve(EntryRef::Command(\"center_half\")) must return a Command entry; got kind {:?}",
            found.kind()
        );
        match found {
            Entry::Command(c) => assert_eq!(
                c.kind,
                CommandKind::CenterHalf,
                "resolved Command must have kind CenterHalf; got {:?}",
                c.kind
            ),
            other => panic!("expected Entry::Command, got {other:?}"),
        }

        // Cross-variant guard: an Application reference for "center" must NOT
        // resolve to the Command::Center entry (different EntryRef variant).
        let app_ref = EntryRef::Application("center".into());
        let resolved_as_app = resolve(&entries, &app_ref);
        // It may match the actual Application named "Center" (desktop_id
        // "center.desktop") — that doesn't match "center" either. We assert
        // that whatever resolves is NOT the Command::Center entry.
        if let Some(found) = resolved_as_app {
            assert_ne!(
                found.kind(),
                EntryKind::Command,
                "EntryRef::Application(\"center\") must NOT resolve to a Command entry; got kind {:?}",
                found.kind()
            );
        }

        // Cross-variant guard (other direction): EntryRef::Command("center")
        // must NOT resolve to the Application whose desktop_id is "center".
        let cmd_ref_center = EntryRef::Command("center".into());
        let resolved_as_cmd = resolve(&entries, &cmd_ref_center)
            .expect("resolve should find a Command for \"center\"");
        assert_eq!(
            resolved_as_cmd.kind(),
            EntryKind::Command,
            "EntryRef::Command(\"center\") must resolve to a Command, not an Application; got kind {:?}",
            resolved_as_cmd.kind()
        );

        // Missing command id returns None.
        let missing = EntryRef::Command("not-a-command".into());
        assert_eq!(
            resolve(&entries, &missing),
            None,
            "resolve should return None for a Command id not in the slice"
        );
    }

    #[test]
    fn entry_ref_command_serializes_to_tagged_json() {
        let r = EntryRef::Command("center_half".into());

        let serialized = serde_json::to_string(&r).expect("EntryRef::Command should serialize");
        assert_eq!(
            serialized, r#"{"type":"command","id":"center_half"}"#,
            "EntryRef::Command should serialize with tag=type/content=id and snake_case variant; got {serialized}"
        );

        let round_tripped: EntryRef =
            serde_json::from_str(&serialized).expect("EntryRef::Command should deserialize");
        assert_eq!(
            round_tripped, r,
            "EntryRef::Command should round-trip via serde_json; got {round_tripped:?}"
        );
    }

    #[test]
    fn entry_command_methods_return_command_data() {
        // Center — display name "Center", icon "focus-windows-symbolic".
        let center = Entry::Command(make_command(CommandKind::Center));
        assert_eq!(
            center.name(),
            "Center",
            "Entry::Command(Center)::name should return the display name \"Center\"; got {:?}",
            center.name()
        );
        assert_eq!(
            center.icon(),
            Some("focus-windows-symbolic"),
            "Entry::Command(Center)::icon should return Some(\"focus-windows-symbolic\"); got {:?}",
            center.icon()
        );
        assert_eq!(
            center.kind(),
            EntryKind::Command,
            "Entry::Command(Center)::kind should return EntryKind::Command; got {:?}",
            center.kind()
        );

        // ToggleMaximize — display name "Toggle maximize", icon
        // "window-maximize-symbolic".
        let toggle_max = Entry::Command(make_command(CommandKind::ToggleMaximize));
        assert_eq!(
            toggle_max.name(),
            "Toggle maximize",
            "Entry::Command(ToggleMaximize)::name should be \"Toggle maximize\"; got {:?}",
            toggle_max.name()
        );
        assert_eq!(
            toggle_max.icon(),
            Some("window-maximize-symbolic"),
            "Entry::Command(ToggleMaximize)::icon should be Some(\"window-maximize-symbolic\"); got {:?}",
            toggle_max.icon()
        );

        // LeftHalf — display name "Left half", icon "view-dual-symbolic".
        let left_half = Entry::Command(make_command(CommandKind::LeftHalf));
        assert_eq!(
            left_half.name(),
            "Left half",
            "Entry::Command(LeftHalf)::name should be \"Left half\"; got {:?}",
            left_half.name()
        );
        assert_eq!(
            left_half.icon(),
            Some("view-dual-symbolic"),
            "Entry::Command(LeftHalf)::icon should be Some(\"view-dual-symbolic\"); got {:?}",
            left_half.icon()
        );
    }

    #[test]
    fn command_kind_id_round_trips_through_from_id() {
        for &kind in ALL_COMMAND_KINDS {
            let id = kind.as_id();
            let parsed = CommandKind::from_id(id);
            assert_eq!(
                parsed,
                Some(kind),
                "CommandKind::from_id({id:?}) should round-trip to Some({kind:?}); got {parsed:?}"
            );
        }

        let unknown = CommandKind::from_id("not-a-command");
        assert_eq!(
            unknown, None,
            "CommandKind::from_id(\"not-a-command\") should be None; got {unknown:?}"
        );
    }

    /// Exhaustive list of all `PowerCommandKind` variants, kept in one place
    /// so the round-trip tests below stay synchronized. If a variant is
    /// added, this list must grow — and the tests will fail loudly until the
    /// maintainer extends it. Mirrors `ALL_COMMAND_KINDS`.
    const ALL_POWER_COMMAND_KINDS: &[PowerCommandKind] = &[
        PowerCommandKind::LockSession,
        PowerCommandKind::Logout,
        PowerCommandKind::Suspend,
        PowerCommandKind::Restart,
        PowerCommandKind::Shutdown,
    ];

    #[test]
    fn entry_power_command_reference_round_trips() {
        for &kind in ALL_POWER_COMMAND_KINDS {
            let entry = Entry::PowerCommand(make_power_command(kind));
            let reference = entry.reference();
            let expected_ref = EntryRef::PowerCommand(kind.as_id().into());
            assert_eq!(
                reference, expected_ref,
                "Entry::PowerCommand({kind:?}).reference() should be EntryRef::PowerCommand(kind.as_id()); got {reference:?}, want {expected_ref:?}"
            );

            let entries = vec![entry.clone()];
            let resolved = resolve(&entries, &entry.reference());
            assert!(
                matches!(resolved, Some(r) if r == &entry),
                "resolve should return Some(&entry) for the PowerCommand reference of kind {kind:?}; got {resolved:?}"
            );
        }
    }

    #[test]
    fn resolve_finds_power_command_by_reference() {
        let entries = vec![
            Entry::Application(make_application("Center", "center.desktop", None)),
            Entry::Window(make_window(7, "A Window", Some("Firefox"), None)),
            Entry::Workspace(make_workspace(0, "Workspace 1")),
            Entry::Command(make_command(CommandKind::Center)),
            Entry::PowerCommand(make_power_command(PowerCommandKind::Suspend)),
            Entry::PowerCommand(make_power_command(PowerCommandKind::LockSession)),
        ];

        // Positive: EntryRef::PowerCommand("suspend") resolves to the Suspend
        // PowerCommand entry.
        let reference = EntryRef::PowerCommand("suspend".into());
        let resolved = resolve(&entries, &reference);
        let found = resolved.expect("resolve should find a PowerCommand for \"suspend\"");
        assert_eq!(
            found.kind(),
            EntryKind::PowerCommand,
            "resolve(EntryRef::PowerCommand(\"suspend\")) must return a PowerCommand entry; got kind {:?}",
            found.kind()
        );
        match found {
            Entry::PowerCommand(c) => assert_eq!(
                c.kind,
                PowerCommandKind::Suspend,
                "resolved PowerCommand must have kind Suspend; got {:?}",
                c.kind
            ),
            other => panic!("expected Entry::PowerCommand, got {other:?}"),
        }

        // Cross-variant guard: EntryRef::Command("suspend") must NOT resolve
        // to the Suspend PowerCommand. The window-Command id space and the
        // PowerCommand id space are distinct EntryRef variants. "suspend" is
        // not a valid CommandKind id, so resolve returns None here — but
        // even if it weren't None, it must never be a PowerCommand.
        let cmd_ref_suspend = EntryRef::Command("suspend".into());
        let resolved_as_cmd = resolve(&entries, &cmd_ref_suspend);
        if let Some(found) = resolved_as_cmd {
            assert_ne!(
                found.kind(),
                EntryKind::PowerCommand,
                "EntryRef::Command(\"suspend\") must NOT resolve to a PowerCommand entry; got kind {:?}",
                found.kind()
            );
        }

        // Cross-variant guard (other direction): EntryRef::PowerCommand("center")
        // must NOT resolve to the Center window Command. There is no
        // PowerCommandKind with id "center", so this must be None.
        let power_ref_center = EntryRef::PowerCommand("center".into());
        let resolved_as_power = resolve(&entries, &power_ref_center);
        if let Some(found) = resolved_as_power {
            assert_ne!(
                found.kind(),
                EntryKind::Command,
                "EntryRef::PowerCommand(\"center\") must NOT resolve to a window Command entry; got kind {:?}",
                found.kind()
            );
        }

        // Missing PowerCommand id returns None.
        let missing = EntryRef::PowerCommand("hibernate".into());
        assert_eq!(
            resolve(&entries, &missing),
            None,
            "resolve should return None for a PowerCommand id not in the slice; got {:?}",
            resolve(&entries, &missing)
        );
    }

    #[test]
    fn entry_ref_power_command_serializes_to_tagged_json() {
        let r = EntryRef::PowerCommand("suspend".into());

        let serialized =
            serde_json::to_string(&r).expect("EntryRef::PowerCommand should serialize");
        assert_eq!(
            serialized, r#"{"type":"power_command","id":"suspend"}"#,
            "EntryRef::PowerCommand should serialize with tag=type/content=id and snake_case variant; got {serialized}"
        );

        let round_tripped: EntryRef =
            serde_json::from_str(&serialized).expect("EntryRef::PowerCommand should deserialize");
        assert_eq!(
            round_tripped, r,
            "EntryRef::PowerCommand should round-trip via serde_json; got {round_tripped:?}"
        );
    }

    #[test]
    fn entry_power_command_methods_return_command_data() {
        // LockSession — display name "Lock", icon "system-lock-screen-symbolic".
        let lock = Entry::PowerCommand(make_power_command(PowerCommandKind::LockSession));
        assert_eq!(
            lock.name(),
            "Lock",
            "Entry::PowerCommand(LockSession)::name should be \"Lock\"; got {:?}",
            lock.name()
        );
        assert_eq!(
            lock.icon(),
            Some("system-lock-screen-symbolic"),
            "Entry::PowerCommand(LockSession)::icon should be Some(\"system-lock-screen-symbolic\"); got {:?}",
            lock.icon()
        );
        assert_eq!(
            lock.kind(),
            EntryKind::PowerCommand,
            "Entry::PowerCommand(LockSession)::kind should return EntryKind::PowerCommand; got {:?}",
            lock.kind()
        );

        // Suspend — display name "Suspend", icon "weather-clear-night-symbolic".
        let suspend = Entry::PowerCommand(make_power_command(PowerCommandKind::Suspend));
        assert_eq!(
            suspend.name(),
            "Suspend",
            "Entry::PowerCommand(Suspend)::name should be \"Suspend\"; got {:?}",
            suspend.name()
        );
        assert_eq!(
            suspend.icon(),
            Some("weather-clear-night-symbolic"),
            "Entry::PowerCommand(Suspend)::icon should be Some(\"weather-clear-night-symbolic\"); got {:?}",
            suspend.icon()
        );

        // Shutdown — display name "Shutdown", icon "system-shutdown-symbolic".
        let shutdown = Entry::PowerCommand(make_power_command(PowerCommandKind::Shutdown));
        assert_eq!(
            shutdown.name(),
            "Shutdown",
            "Entry::PowerCommand(Shutdown)::name should be \"Shutdown\"; got {:?}",
            shutdown.name()
        );
        assert_eq!(
            shutdown.icon(),
            Some("system-shutdown-symbolic"),
            "Entry::PowerCommand(Shutdown)::icon should be Some(\"system-shutdown-symbolic\"); got {:?}",
            shutdown.icon()
        );
    }

    #[test]
    fn power_command_kind_id_round_trips_through_from_id() {
        for &kind in ALL_POWER_COMMAND_KINDS {
            let id = kind.as_id();
            let parsed = PowerCommandKind::from_id(id);
            assert_eq!(
                parsed,
                Some(kind),
                "PowerCommandKind::from_id({id:?}) should round-trip to Some({kind:?}); got {parsed:?}"
            );
        }

        let unknown = PowerCommandKind::from_id("not-a-power-command");
        assert_eq!(
            unknown, None,
            "PowerCommandKind::from_id(\"not-a-power-command\") should be None; got {unknown:?}"
        );
    }
}
