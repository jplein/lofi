use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::pango;
use lofi_core::{Entry, EntryKind, search};

use crate::launch;

const WINDOW_WIDTH: i32 = 480;
const WINDOW_HEIGHT: i32 = 500;
const WINDOW_PADDING: i32 = 12;
const ICON_SIZE: i32 = 24;
const LIST_MARGIN: i32 = 4;
const ROW_SPACING: i32 = 8;
const ROW_MARGIN_H: i32 = 8;
const ROW_MARGIN_V: i32 = 4;

/// Internal launcher state. `entries` is the full gathered set; `visible`
/// holds indices into `entries` in the order currently shown in the list.
struct UiState {
    entries: Vec<Entry>,
    visible: Vec<usize>,
}

/// Build and present the launcher window. Takes ownership of `entries`; the
/// caller hands us a fresh gather and we do not refresh it during the window's
/// lifetime.
pub fn build(app: &adw::Application, entries: Vec<Entry>) {
    let search_entry = gtk::SearchEntry::builder()
        .hexpand(true)
        .margin_top(LIST_MARGIN)
        .margin_bottom(LIST_MARGIN)
        .margin_start(LIST_MARGIN)
        .margin_end(LIST_MARGIN)
        .build();

    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
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

    let state = Rc::new(RefCell::new(UiState {
        entries,
        visible: Vec::new(),
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
        search_entry.connect_activate(move |_| {
            if let Some(entry) = selected_entry(&list_box, &state) {
                launch::activate(&entry);
                window.close();
            }
        });
    }

    // Up/Down navigate the list; Escape closes. Enter is intentionally absent
    // here — see the connect_activate block above.
    let key_controller = gtk::EventControllerKey::new();
    {
        let list_box = list_box.clone();
        let window = window.clone();
        key_controller.connect_key_pressed(
            move |_ctrl, keyval, _keycode, _modifiers| match keyval {
                gtk::gdk::Key::Up => {
                    move_selection(&list_box, -1);
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Down => {
                    move_selection(&list_box, 1);
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
}

/// Move the list selection by `delta` (typically +/-1). No-op when no row is
/// selected or the new index is out of range. Focus stays on the search entry.
fn move_selection(list_box: &gtk::ListBox, delta: i32) {
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
/// the full set; otherwise we run the fuzzy matcher and translate result refs
/// back into indices via pointer equality against the owning vec.
fn populate_list(list_box: &gtk::ListBox, state: &Rc<RefCell<UiState>>, query: &str) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let new_visible: Vec<usize> = if query.trim().is_empty() {
        let s = state.borrow();
        (0..s.entries.len()).collect()
    } else {
        let s = state.borrow();
        let matched = search(&s.entries, query);
        let mut indices = Vec::with_capacity(matched.len());
        for m in matched {
            for (i, e) in s.entries.iter().enumerate() {
                if std::ptr::eq(e, m) {
                    indices.push(i);
                    break;
                }
            }
        }
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

/// Build a single list row showing icon + name + kind.
fn build_row(entry: &Entry) -> gtk::ListBoxRow {
    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(ROW_SPACING)
        .margin_start(ROW_MARGIN_H)
        .margin_end(ROW_MARGIN_H)
        .margin_top(ROW_MARGIN_V)
        .margin_bottom(ROW_MARGIN_V)
        .build();

    let image = match entry.icon() {
        Some(s) if s.starts_with('/') => gtk::Image::from_file(Path::new(s)),
        Some(s) => gtk::Image::from_icon_name(s),
        None => gtk::Image::new(),
    };
    image.set_pixel_size(ICON_SIZE);
    hbox.append(&image);

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
    }
}
