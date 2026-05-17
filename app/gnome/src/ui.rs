use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use gtk::glib;
use gtk::pango;
use lofi_core::{Entry, EntryKind, EntryRef, MruStore, search};

use crate::launch;

const WINDOW_WIDTH: i32 = 480;
const WINDOW_HEIGHT: i32 = 500;
const WINDOW_PADDING: i32 = 12;
const ICON_SIZE: i32 = 24;
const LIST_MARGIN: i32 = 4;
// Extra breathing room under the search field so the larger text in it
// doesn't visually crowd the first row of the list.
const SEARCH_ENTRY_BOTTOM_MARGIN: i32 = 12;
const ROW_SPACING: i32 = 8;
const ROW_MARGIN_H: i32 = 8;
// Asymmetric vertical margins (sum preserved at 8px so row height is
// unchanged from the original 4/4 split) — the icon column's dot at the
// bottom pulls the visual centre of mass up a hair, so we compensate by
// pushing content down.
const ROW_MARGIN_TOP: i32 = 6;
const ROW_MARGIN_BOTTOM: i32 = 2;
const RUNNING_DOT_SIZE: i32 = 6;
const ICON_COLUMN_SPACING: i32 = 2;

/// App-wide CSS for the launcher. Covers:
///
/// 1. The running-indicator dot under an Application's icon when
///    `recent_window_id.is_some()`. `alpha(@theme_fg_color, ...)` adapts to
///    light/dark themes; `border-radius: 9999px` forces a circle regardless
///    of the box's actual dimensions.
/// 2. The top SearchEntry, stripped of its default rounded border, focus
///    ring, and tinted fill so it blends into the window background instead
///    of looking like a separate input control inset into the chrome.
const LAUNCHER_CSS: &str = "\
.running-indicator {
    background-color: alpha(@theme_fg_color, 0.8);
    border-radius: 9999px;
    min-width: 6px;
    min-height: 6px;
}
/* GtkSearchEntry's CSS node has been spelled both `searchentry` and
   `entry.search` across GTK4 versions. We target both, plus the inner `text`
   node where the focus ring actually lives in GTK4.14+, so the input blends
   into the window background regardless of the precise GTK build. The `.flat`
   style class added to the widget (see `build()`) handles the frame removal
   on its own; this rule reinforces it and strips the tinted fill that
   `.flat` doesn't touch. */
searchentry,
searchentry > text,
entry.search,
entry.search > text {
    background-color: transparent;
    background-image: none;
    box-shadow: none;
    border: none;
    outline: none;
}
/* Shift the magnifying-glass icon (and the text that follows it) inward so
   the icon's right edge visually aligns with the list rows' icon column.
   The list's icon column sits at `WINDOW_PADDING + ROW_MARGIN_H` from the
   window edge; the SearchEntry container sits at `WINDOW_PADDING +
   LIST_MARGIN`. We override the entry's leading padding rather than adding a
   margin to the inner image — selectors like `searchentry > image` don't
   reliably match because GTK's SearchEntry wraps its icon in an internal
   GtkBox whose CSS-node layout has shifted across GTK4 versions. */
searchentry,
entry.search {
    padding-left: 10px;
}
/* Nudge the typed text right so it aligns with the list rows' name labels.
   The row label sits at WINDOW_PADDING + ROW_MARGIN_H + ICON_SIZE + ROW_SPACING
   from the window edge; the SearchEntry's default gap between its leading
   image and the text widget falls a couple pixels short of that target. */
searchentry > text,
entry.search > text {
    padding-left: 4px;
}
/* Adwaita gives the window chrome `@window_bg_color` and the ListBox
   `@view_bg_color` (the canonical content-surface tone), which differ by a
   small amount. Pulling the window background up to `@view_bg_color`
   eliminates that seam so the whole launcher reads as one surface. */
window {
    background-color: @view_bg_color;
}
";

/// Latch ensuring `install_styles` only registers our provider with the
/// default display once per process. `build()` runs on every
/// `connect_activate`, but re-registering the same provider is wasted work
/// (and would stack identical priority entries).
static STYLES_INSTALLED: OnceLock<()> = OnceLock::new();

/// Register the running-indicator CSS once per process. Called from `build()`
/// because we need a live default `gdk::Display`, which only exists after
/// `adw::Application::activate` fires. Guarded by `STYLES_INSTALLED` so
/// repeat invocations are no-ops. Returns silently if there's no default
/// display (headless tests, broken environment) — the dot just won't be
/// styled and falls back to whatever the GTK default theme renders for an
/// empty `gtk::Box`.
fn install_styles() {
    if STYLES_INSTALLED.get().is_some() {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    // `load_from_string` is gated behind gtk4's `v4_12` feature; we target
    // the unfeatured baseline so use `load_from_data`, which is the same
    // call with a different signature.
    provider.load_from_data(LAUNCHER_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let _ = STYLES_INSTALLED.set(());
}

/// Internal launcher state. `entries` is the full gathered set; `visible`
/// holds indices into `entries` in the order currently shown in the list.
/// `mru_position` maps each known `EntryRef` to its rank in the persisted
/// recency index (0 = most recent); entries absent from the map fall to the
/// bottom of the displayed list in input order.
struct UiState {
    entries: Vec<Entry>,
    visible: Vec<usize>,
    mru_position: HashMap<EntryRef, usize>,
}

/// Build and present the launcher window. Takes ownership of `entries`; the
/// caller hands us a fresh gather and we do not refresh it during the
/// window's lifetime. `mru_store` is `None` when the store could not be
/// opened (e.g. no XDG_STATE_HOME and no HOME) — sorting still happens
/// against `mru_index`, only the on-activation bump is skipped.
pub fn build(
    app: &adw::Application,
    entries: Vec<Entry>,
    mru_store: Option<Rc<MruStore>>,
    mru_index: Vec<EntryRef>,
) {
    install_styles();

    let search_entry = gtk::SearchEntry::builder()
        .hexpand(true)
        .margin_top(LIST_MARGIN)
        .margin_bottom(SEARCH_ENTRY_BOTTOM_MARGIN)
        .margin_start(LIST_MARGIN)
        .margin_end(LIST_MARGIN)
        .build();
    // `.flat` is GTK's built-in style class for frameless entries. Adwaita
    // honours it on GtkSearchEntry; supplemental CSS in `LAUNCHER_CSS` strips
    // the tinted fill and focus shadow that `.flat` alone leaves behind.
    search_entry.add_css_class("flat");

    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(true)
        .build();

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&list_box)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(WINDOW_PADDING)
        .margin_bottom(WINDOW_PADDING)
        .margin_start(WINDOW_PADDING)
        .margin_end(WINDOW_PADDING)
        .build();
    content.append(&search_entry);
    content.append(&scroller);

    // No `decorated(false)`: keep client-side decorations so we get the GTK
    // drop shadow and rounded-corner clipping. AdwApplicationWindow has no
    // titlebar by default, so we don't get one even with decorations on.
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("LoFi")
        .default_width(WINDOW_WIDTH)
        .default_height(WINDOW_HEIGHT)
        .resizable(false)
        .modal(true)
        .build();
    window.set_content(Some(&content));

    // Build the MRU-rank lookup once. The persisted index is already in
    // most-recent-first order, so its enumerated position is the rank.
    let mru_position: HashMap<EntryRef, usize> = mru_index
        .into_iter()
        .enumerate()
        .map(|(rank, r)| (r, rank))
        .collect();

    let state = Rc::new(RefCell::new(UiState {
        entries,
        visible: Vec::new(),
        mru_position,
    }));

    // Wire search-changed: rebuild list from the current query.
    {
        let state = state.clone();
        let list_box = list_box.clone();
        search_entry.connect_search_changed(move |entry| {
            let query = entry.text();
            populate_list(&list_box, &state, query.as_str());
        });
    }

    // Enter is handled via SearchEntry's `activate` signal because gtk::Entry's
    // default key-pressed handler consumes Return in the target phase, so a
    // bubble-phase EventControllerKey would never see it.
    {
        let state = state.clone();
        let list_box = list_box.clone();
        let window = window.clone();
        let mru_store = mru_store.clone();
        search_entry.connect_activate(move |_| {
            if let Some(entry) = selected_entry(&list_box, &state) {
                bump_mru(mru_store.as_deref(), &entry);
                launch::activate(&entry);
                window.close();
            }
        });
    }

    // Clicking a row activates the underlying entry. The row passed to the
    // signal — not list_box.selected_row() — is the authoritative source, and
    // guards against a stale selection or the non-selectable "No matches" row.
    {
        let state = state.clone();
        let window = window.clone();
        let mru_store = mru_store.clone();
        list_box.connect_row_activated(move |_lb, row| {
            let Ok(row_idx) = usize::try_from(row.index()) else {
                return;
            };
            let entry = {
                let s = state.borrow();
                let Some(&entry_idx) = s.visible.get(row_idx) else {
                    return;
                };
                let Some(entry) = s.entries.get(entry_idx) else {
                    return;
                };
                entry.clone()
            };
            bump_mru(mru_store.as_deref(), &entry);
            launch::activate(&entry);
            window.close();
        });
    }

    // Up/Down navigate the list; Escape closes. Enter is intentionally absent
    // here — see the connect_activate block above.
    let key_controller = gtk::EventControllerKey::new();
    {
        let list_box = list_box.clone();
        let scroller = scroller.clone();
        let window = window.clone();
        key_controller.connect_key_pressed(
            move |_ctrl, keyval, _keycode, _modifiers| match keyval {
                gtk::gdk::Key::Up => {
                    move_selection(&list_box, &scroller, -1);
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Down => {
                    move_selection(&list_box, &scroller, 1);
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Escape => {
                    window.close();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            },
        );
    }
    search_entry.add_controller(key_controller);

    populate_list(&list_box, &state, "");

    window.present();
    // Without this, the AdwApplicationWindow comes up with no focused widget
    // and the user has to click the SearchEntry before typing reaches it.
    // Call after `present()` so the widget is realised — `grab_focus` on an
    // unrealised widget is silently a no-op.
    search_entry.grab_focus();
}

/// Move the list selection by `delta` (typically +/-1). No-op when no row is
/// selected or the new index is out of range. Focus stays on the search entry.
fn move_selection(list_box: &gtk::ListBox, scroller: &gtk::ScrolledWindow, delta: i32) {
    let current = match list_box.selected_row().map(|r| r.index()) {
        Some(i) if i >= 0 => i,
        _ => return,
    };
    let target = current + delta;
    if target < 0 {
        return;
    }
    if let Some(row) = list_box.row_at_index(target) {
        list_box.select_row(Some(&row));
        scroll_row_into_view(list_box, scroller, &row);
    }
}

/// Scroll `scroller` so `row` is fully visible. GTK only auto-scrolls on focus
/// changes; we move selection programmatically without shifting focus from the
/// SearchEntry, so we have to nudge the adjustment.
fn scroll_row_into_view(
    list_box: &gtk::ListBox,
    scroller: &gtk::ScrolledWindow,
    row: &gtk::ListBoxRow,
) {
    let Some(bounds) = row.compute_bounds(list_box) else {
        return;
    };
    let vadj = scroller.vadjustment();
    let row_top = f64::from(bounds.y());
    let row_bottom = row_top + f64::from(bounds.height());
    let visible_top = vadj.value();
    let visible_bottom = visible_top + vadj.page_size();
    if row_top < visible_top {
        vadj.set_value(row_top);
    } else if row_bottom > visible_bottom {
        vadj.set_value(row_bottom - vadj.page_size());
    }
}

/// Pull the `Entry` corresponding to the currently selected list row out of
/// state. Scoped so the borrow is released before the caller does anything
/// else with `state`.
fn selected_entry(list_box: &gtk::ListBox, state: &Rc<RefCell<UiState>>) -> Option<Entry> {
    let row = list_box.selected_row()?;
    let idx_i32 = row.index();
    let row_idx = usize::try_from(idx_i32).ok()?;
    let s = state.borrow();
    let entry_idx = *s.visible.get(row_idx)?;
    s.entries.get(entry_idx).cloned()
}

/// Rebuild the list rows from `query`. Empty/whitespace queries pass through
/// the full set; otherwise we run the fuzzy matcher (filter-only) and translate
/// result refs back into indices via pointer equality against the owning vec.
/// The resulting index list is then stably sorted by MRU rank — entries in the
/// persisted recency index rise to the top in most-recent-first order; entries
/// absent from the index fall to the bottom in input order. Stable selection
/// during typing is the whole point: see the MRU plan for context.
fn populate_list(list_box: &gtk::ListBox, state: &Rc<RefCell<UiState>>, query: &str) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let new_visible: Vec<usize> = {
        let s = state.borrow();
        let mut indices: Vec<usize> = if query.trim().is_empty() {
            (0..s.entries.len()).collect()
        } else {
            let matched = search(&s.entries, query);
            let mut idxs = Vec::with_capacity(matched.len());
            for m in matched {
                for (i, e) in s.entries.iter().enumerate() {
                    if std::ptr::eq(e, m) {
                        idxs.push(i);
                        break;
                    }
                }
            }
            idxs
        };
        // Stable sort: in-MRU entries rise in MRU order; non-MRU entries (rank
        // usize::MAX) keep their relative input order at the bottom.
        indices.sort_by_key(|i| {
            s.entries
                .get(*i)
                .map(|e| {
                    s.mru_position
                        .get(&e.reference())
                        .copied()
                        .unwrap_or(usize::MAX)
                })
                .unwrap_or(usize::MAX)
        });
        indices
    };

    if new_visible.is_empty() && !query.trim().is_empty() {
        let label = gtk::Label::builder()
            .label("No matches")
            .halign(gtk::Align::Center)
            .build();
        label.add_css_class("dim-label");
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&label));
        row.set_selectable(false);
        list_box.append(&row);
    } else {
        let s = state.borrow();
        for &idx in &new_visible {
            if let Some(entry) = s.entries.get(idx) {
                let row = build_row(entry);
                list_box.append(&row);
            }
        }
        if let Some(first) = list_box.row_at_index(0) {
            list_box.select_row(Some(&first));
        }
    }

    drop(std::mem::replace(
        &mut state.borrow_mut().visible,
        new_visible,
    ));
}

/// Build a single list row showing icon + name + kind. For running
/// Applications (`recent_window_id.is_some()`) a small CSS-styled dot is
/// drawn directly under the icon, mirroring the GNOME dock's
/// running-indicator. The dot widget is always added but hidden via
/// `set_visible(false)` for non-running entries so all rows share the same
/// vertical layout and the icon column doesn't shift between rows.
fn build_row(entry: &Entry) -> gtk::ListBoxRow {
    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(ROW_SPACING)
        .margin_start(ROW_MARGIN_H)
        .margin_end(ROW_MARGIN_H)
        .margin_top(ROW_MARGIN_TOP)
        .margin_bottom(ROW_MARGIN_BOTTOM)
        .build();

    let icon_column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(ICON_COLUMN_SPACING)
        .valign(gtk::Align::Center)
        .build();

    let image = match entry.icon() {
        Some(s) if s.starts_with('/') => gtk::Image::from_file(Path::new(s)),
        Some(s) => gtk::Image::from_icon_name(s),
        None => gtk::Image::new(),
    };
    image.set_pixel_size(ICON_SIZE);
    icon_column.append(&image);

    // The dot is always present at the same size so every row's icon column
    // has identical height; only the visible styling (`.running-indicator`
    // class) is applied for running apps. `set_visible(false)` would drop the
    // widget from layout entirely and make rows shift height between
    // running/non-running entries.
    let dot = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .build();
    dot.set_size_request(RUNNING_DOT_SIZE, RUNNING_DOT_SIZE);
    if matches!(entry, Entry::Application(a) if a.recent_window_id.is_some()) {
        dot.add_css_class("running-indicator");
    }
    icon_column.append(&dot);

    hbox.append(&icon_column);

    let name_label = gtk::Label::builder()
        .label(entry.name())
        .halign(gtk::Align::Start)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .single_line_mode(true)
        .xalign(0.0)
        .build();
    hbox.append(&name_label);

    let kind_label = gtk::Label::builder()
        .label(kind_to_str(entry.kind()))
        .halign(gtk::Align::End)
        .build();
    kind_label.add_css_class("dim-label");
    hbox.append(&kind_label);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&hbox));
    row
}

/// Human-readable label for an entry's `EntryKind`. Exhaustive so a new
/// variant forces an update here.
fn kind_to_str(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Application => "Application",
        EntryKind::Window => "Window",
        EntryKind::Workspace => "Workspace",
        EntryKind::Command => "Command",
    }
}

/// Best-effort: bump `entry`'s ref in the persistent MRU store. Called from
/// both activation paths (Enter and click) right before `launch::activate`.
/// `store` is `None` when the SQLite file could not be opened; bump errors
/// log via `eprintln!` and are otherwise swallowed because there is no
/// useful caller-side recovery — the launch still happens.
fn bump_mru(store: Option<&MruStore>, entry: &Entry) {
    if let Some(store) = store
        && let Err(e) = store.bump(&entry.reference())
    {
        eprintln!("mru: bump failed for {}: {e}", entry.name());
    }
}
