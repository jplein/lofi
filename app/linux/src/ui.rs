use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use gtk::glib;
use gtk::pango;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use lofi_core::{Entry, EntryKind, EntryRef, MruStore};

use crate::backend::Backend;
use crate::config::{Appearance, Shadow};
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
/// CSS class on the frame widget that carries the launcher's surface colour,
/// corner radius, border, and (when configured) drop shadow on the
/// layer-shell path. See `build` and `launcher_css`.
const FRAME_CSS_CLASS: &str = "lofi-frame";

/// The config-independent half of the launcher's CSS. Covers:
///
/// 1. The running-indicator dot under an Application's icon when
///    `recent_window_id.is_some()`. `alpha(@theme_fg_color, ...)` adapts to
///    light/dark themes; `border-radius: 9999px` forces a circle regardless
///    of the box's actual dimensions.
/// 2. The top SearchEntry, stripped of its default rounded border, focus
///    ring, and tinted fill so it blends into the window background instead
///    of looking like a separate input control inset into the chrome.
/// 3. The list chain, forced transparent so the one surface shows through it.
///
/// Which node actually *paints* that surface is the config-dependent half,
/// and lives in [`launcher_css`].
const BASE_CSS: &str = "\
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
/* GTK gives a bare `list` — and the viewport/scroller wrapping it — the VIEW
   background, which would punch a panel of a different tone through the
   popover surface underneath (painted by `window` on the toplevel path, by
   `.lofi-frame` on the layer-shell one). Forcing the whole chain transparent is
   what keeps the launcher reading as one surface. The viewport is named
   because it is a node of that same chain: it paints nothing under Adwaita
   today, so naming it costs no pixel, and it is what stops a theme that does
   give it a background from reintroducing the panel one level down. */
scrolledwindow,
scrolledwindow > viewport,
list {
    background-color: transparent;
}
";

/// Surface paint for the ordinary-toplevel path (GNOME).
///
/// The launcher is conceptually a menu, not a document window, so it takes the
/// popover surface rather than the window or view one. `@popover_bg_color` /
/// `@popover_fg_color` are libadwaita named colours, so this follows the
/// light/dark scheme without naming a literal.
///
/// Nothing here rounds a corner or draws a shadow, because on this path GTK's
/// client-side decorations already do both.
const PLAIN_SURFACE_CSS: &str = "\
window {
    background-color: @popover_bg_color;
    color: @popover_fg_color;
}
";

/// The launcher's full stylesheet: [`BASE_CSS`] plus whichever surface paint
/// the presentation path calls for.
///
/// `appearance` is `Some` only on the layer-shell path, where the window has
/// no decorations of its own to inherit (`libgtk4-layer-shell` calls
/// `gtk_window_set_decorated(FALSE)` on the window it converts) and the
/// compositor cannot supply a border or round the surface either. There, the
/// surface colour moves off the `window` node and onto a frame widget
/// (`.lofi-frame`, see `build`) that can carry a radius, a border, and a
/// shadow; the window itself goes transparent so the corners it no longer
/// paints show the desktop through. `None` leaves the toplevel path exactly as
/// it was.
///
/// Pure and separate from [`install_styles`] so the generated declarations are
/// unit-testable without a `gdk::Display` — the same split as
/// `config::parse` / `config::load`.
fn launcher_css(appearance: Option<&Appearance>) -> String {
    let Some(appearance) = appearance else {
        return format!("{BASE_CSS}{PLAIN_SURFACE_CSS}");
    };

    let mut css = String::from(BASE_CSS);
    css.push_str(
        "\
window {
    background-color: transparent;
    color: @popover_fg_color;
}
",
    );

    // `write!` into a String is infallible, so the Results are discarded
    // rather than unwrapped — this module has no error path to report into.
    let _ = writeln!(
        css,
        ".{FRAME_CSS_CLASS} {{\n    background-color: @popover_bg_color;\n    \
         border-radius: {}px;",
        appearance.corner_radius
    );

    // Omitted entirely rather than emitted as `0px solid`, so the default
    // config adds no declaration at all.
    if appearance.border_width > 0 {
        let _ = writeln!(
            css,
            "    border: {}px solid {};",
            appearance.border_width,
            appearance.border_color.as_css()
        );
    }

    // Niri defines its shadow parameters by reference to CSS box-shadow —
    // `softness` is the blur radius, `spread` the spread, `offset` the offset
    // — so this is a transcription, not an approximation. See `config`.
    if let Some(shadow) = appearance.shadow.as_ref() {
        let _ = writeln!(
            css,
            "    box-shadow: {}px {}px {}px {}px {};",
            shadow.offset.x,
            shadow.offset.y,
            shadow.softness,
            shadow.spread,
            shadow.color.as_css()
        );
    }

    css.push_str("}\n");
    css
}

/// Latch ensuring `install_styles` only registers our provider with the
/// default display once per process. `build()` runs on every
/// `connect_activate`, but re-registering the same provider is wasted work
/// (and would stack identical priority entries). Safe despite `install_styles`
/// now taking an argument: both the backend and the config file are read once
/// in `main`, so the stylesheet cannot differ between calls within a process.
static STYLES_INSTALLED: OnceLock<()> = OnceLock::new();

/// Register the launcher's CSS once per process. Called from `build()`
/// because we need a live default `gdk::Display`, which only exists after
/// `adw::Application::activate` fires. Guarded by `STYLES_INSTALLED` so
/// repeat invocations are no-ops. Returns silently if there's no default
/// display (headless tests, broken environment) — the dot just won't be
/// styled and falls back to whatever the GTK default theme renders for an
/// empty `gtk::Box`.
fn install_styles(appearance: Option<&Appearance>) {
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
    provider.load_from_data(&launcher_css(appearance));
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
///
/// `backend` is threaded through to the two activation closures (Enter and
/// click) so `launch::activate` can dispatch against the running desktop, and
/// is consulted once for how to present the window (see
/// `configure_layer_shell`). Everything else here is desktop-agnostic.
///
/// `appearance` is the user's configured border, corner radius, and shadow. It
/// is honoured **only** on the layer-shell path: those three are decorations
/// GTK's client-side decorations already supply on an ordinary toplevel, and
/// drawing a second rounded, bordered frame inside one would double up. See
/// `config` for why the layer-shell path has none of its own.
pub fn build(
    app: &adw::Application,
    entries: Vec<Entry>,
    mru_store: Option<Rc<MruStore>>,
    mru_index: Vec<EntryRef>,
    backend: Rc<dyn Backend>,
    appearance: &Appearance,
) {
    let appearance = backend.uses_layer_shell().then_some(appearance);
    install_styles(appearance);

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

    // Transparent room on every side for a LoFi-drawn shadow to blur into;
    // zero whenever nothing is drawing one, which is both the GNOME path and
    // the (default) layer-shell path where Niri draws the shadow itself.
    let shadow_margin = appearance
        .and_then(|a| a.shadow.as_ref())
        .map_or(0, Shadow::margin);

    // No `decorated(false)`: on an ordinary toplevel, client-side decorations
    // are what give us the GTK drop shadow and rounded-corner clipping.
    // AdwApplicationWindow has no titlebar by default, so we don't get one
    // even with decorations on.
    //
    // That only holds on the toplevel path. `libgtk4-layer-shell` calls
    // `gtk_window_set_decorated(FALSE)` on the window it converts, so under
    // Niri there is no decoration node, no shadow, and no rounding — which is
    // exactly what the `.lofi-frame` wrapper below replaces.
    //
    // The default size is the *surface*, so it has to include the shadow
    // margin; the frame inside it stays WINDOW_WIDTH x WINDOW_HEIGHT.
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("LoFi")
        .default_width(WINDOW_WIDTH + 2 * shadow_margin)
        .default_height(WINDOW_HEIGHT + 2 * shadow_margin)
        .resizable(false)
        .modal(true)
        .build();

    if appearance.is_some() {
        // `Overflow::Hidden` pushes a *rounded* clip (GTK derives it from the
        // node's CSS padding box), which is what keeps a selected row's
        // highlight from squaring off a corner it happens to reach.
        let frame = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(shadow_margin)
            .margin_bottom(shadow_margin)
            .margin_start(shadow_margin)
            .margin_end(shadow_margin)
            .build();
        frame.add_css_class(FRAME_CSS_CLASS);
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.append(&content);
        window.set_content(Some(&frame));
    } else {
        window.set_content(Some(&content));
    }

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

    // Rebuild the list from the current query on every keystroke.
    //
    // We use `changed` (from GtkEditable), *not* GtkSearchEntry's own
    // `search-changed`. `search-changed` is deliberately debounced — GTK
    // holds it for ~150ms after the last keypress before emitting — which
    // shows up as a visible lag between typing and the list updating.
    // Setting `GtkSearchEntry:search-delay` to 0 would also fix it, but
    // that property needs the gtk4-rs `v4_8` feature and this crate targets
    // the unfeatured GTK baseline (see `install_styles`). `changed` fires
    // synchronously per keystroke with no feature gate; the rebuild is
    // cheap at application-gather scale, so immediate filtering is fine.
    {
        let state = state.clone();
        let list_box = list_box.clone();
        search_entry.connect_changed(move |entry| {
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
        let backend = backend.clone();
        search_entry.connect_activate(move |_| {
            if let Some(entry) = selected_entry(&list_box, &state) {
                bump_mru(mru_store.as_deref(), &entry);
                launch::activate(backend.as_ref(), &entry);
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
        let backend = backend.clone();
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
            launch::activate(backend.as_ref(), &entry);
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

    if backend.uses_layer_shell() {
        configure_layer_shell(&window);
    }

    // Close when the window loses keyboard focus. This is the simplest
    // available substitute for "don't show in Alt-Tab" on GNOME/Wayland —
    // the window only exists while focused, so Alt-Tabbing away closes it
    // before the switcher would even surface it. `is_visible()` guards the
    // teardown notify (close → is_visible false → is_active notify false),
    // preventing a redundant second close call.
    window.connect_is_active_notify(|w| {
        if !w.is_active() && w.is_visible() {
            w.close();
        }
    });

    window.present();
    // Without this, the AdwApplicationWindow comes up with no focused widget
    // and the user has to click the SearchEntry before typing reaches it.
    // Call after `present()` so the widget is realised — `grab_focus` on an
    // unrealised widget is silently a no-op.
    search_entry.grab_focus();
}

/// Turn the launcher window into a `wlr-layer-shell` overlay surface.
///
/// Called only when the backend asks for it — i.e. under Niri, where an
/// ordinary toplevel would be tiled into the scrolling layout and shove the
/// user's windows aside every time the launcher opens. A layer surface is not
/// a toplevel: it floats above everything, never enters the layout, and never
/// appears in the compositor's own window list (which is a bonus — it means
/// the launcher can't turn up as a row in its own window list, and can't be
/// picked as the target of its own window commands).
///
/// Must run before `present()`: `init_layer_shell` reconfigures how the
/// surface is created, so it has no effect once the window is realised.
///
/// Configuration:
///
/// - **Overlay layer** — above ordinary windows *and* above fullscreen ones.
///   Niri renders a focused fullscreen window over the `Top` layer, so `Top`
///   would leave the launcher invisible exactly when a video or a game is
///   full-screen, which is a moment you very much want a launcher.
/// - **Exclusive keyboard** — the surface takes keyboard focus outright.
///   `OnDemand` would require a click first, which defeats "type immediately".
/// - **No anchors** — a layer surface anchored to no edge is centred by the
///   compositor at the window's own requested size, which is exactly the
///   placement we want and saves computing a margin against an output whose
///   geometry we'd have to fetch.
///
/// Silently does nothing when the compositor doesn't advertise layer-shell.
/// That should be unreachable (only the Niri backend asks, and Niri
/// implements the protocol) but the check is free, and the failure mode
/// without it is a GTK-level abort rather than a launcher that comes up as an
/// ordinary window.
fn configure_layer_shell(window: &adw::ApplicationWindow) {
    if !gtk4_layer_shell::is_supported() {
        eprintln!("lofi: compositor does not support wlr-layer-shell; using a plain window");
        return;
    }

    window.init_layer_shell();
    // The namespace is what the compositor sees this surface as — it's the
    // handle a user needs to write a `layer-rule` for the launcher, and it's
    // what shows up in `niri msg layers`.
    window.set_namespace(Some("lofi"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
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

/// Rebuild the list rows from `query`. Ranking is delegated entirely to
/// `lofi_core::rank`: filtering (intersection semantics), the two-tier
/// MRU/score order, and the in-MRU prefix sub-ordering all live in core, so
/// GNOME and macOS share one implementation. An empty/whitespace query is a
/// passthrough in MRU order (recent first, then never-used in input order); a
/// non-empty query that matches nothing yields an empty index list, which we
/// render as the single non-selectable "No matches" row.
fn populate_list(list_box: &gtk::ListBox, state: &Rc<RefCell<UiState>>, query: &str) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let new_visible: Vec<usize> = {
        let s = state.borrow();
        lofi_core::rank(&s.entries, query, &s.mru_position)
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
        EntryKind::PowerCommand => "Power",
        // Workspace-move commands are window-action commands (they act on the
        // captured target window), so they share the "Command" category label
        // with center/minimize/etc. rather than "Workspace" (which is reserved
        // for the switch-to-workspace entries).
        EntryKind::WorkspaceCommand => "Command",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Color, Offset};

    /// The declarations inside `.lofi-frame`, so a test can assert on the
    /// generated rule without depending on the surrounding whitespace.
    fn frame_rule(css: &str) -> String {
        let start = css
            .find(".lofi-frame {")
            .expect("the layer-shell stylesheet should define a frame rule");
        let rest = &css[start..];
        let end = rest.find('}').expect("the frame rule should be closed");
        rest[..=end].to_owned()
    }

    #[test]
    fn the_toplevel_path_is_unchanged_by_the_config() {
        // GNOME's window keeps its CSD shadow and rounding, so none of the
        // appearance settings may reach it — not even the default radius.
        let css = launcher_css(None);
        assert!(
            css.contains("window {\n    background-color: @popover_bg_color;"),
            "the toplevel path must still paint the popover surface on `window`"
        );
        // The rule, not the bare class name — `BASE_CSS` mentions the class in
        // a comment explaining which node paints the surface on each path.
        assert!(
            !css.contains(&format!(".{FRAME_CSS_CLASS} {{")),
            "the toplevel path must not emit a frame rule at all"
        );
        assert!(
            !css.contains("background-color: transparent;\n    color:"),
            "the toplevel path's window must keep painting its own background"
        );
        assert!(
            !css.contains("border-radius: 12px"),
            "the toplevel path must not round anything itself"
        );
    }

    #[test]
    fn the_layer_shell_path_moves_the_surface_onto_the_frame() {
        // The window has to stop painting its corners, or the rounding on the
        // frame would sit on top of an opaque square.
        let css = launcher_css(Some(&Appearance::default()));
        assert!(
            css.contains("window {\n    background-color: transparent;"),
            "the window must go transparent so the frame's corners show through"
        );
        assert!(
            css.contains("color: @popover_fg_color;"),
            "the foreground colour still belongs on `window`, to be inherited"
        );
        assert!(
            frame_rule(&css).contains("background-color: @popover_bg_color;"),
            "the frame is what paints the surface on this path"
        );
    }

    #[test]
    fn the_default_appearance_rounds_but_adds_nothing_else() {
        let rule = frame_rule(&launcher_css(Some(&Appearance::default())));
        assert!(
            rule.contains("border-radius: 12px;"),
            "an unconfigured Niri session should still get libadwaita's radius, got: {rule}"
        );
        assert!(
            !rule.contains("border:"),
            "a zero border width must emit no declaration at all, got: {rule}"
        );
        assert!(
            !rule.contains("box-shadow:"),
            "no shadow unless configured — that is what leaves the surface the \
             right shape for Niri's own layer-rule shadow, got: {rule}"
        );
    }

    #[test]
    fn a_configured_border_and_shadow_are_transcribed() {
        let appearance = Appearance {
            corner_radius: 16,
            border_width: 3,
            border_color: Color::parse("#e0e0e0").expect("fixture colour should parse"),
            shadow: Some(Shadow {
                softness: 30,
                spread: 8,
                offset: Offset { x: 5, y: 2 },
                color: Color::parse("#00000080").expect("fixture colour should parse"),
            }),
        };
        let rule = frame_rule(&launcher_css(Some(&appearance)));

        assert!(rule.contains("border-radius: 16px;"), "got: {rule}");
        assert!(rule.contains("border: 3px solid #e0e0e0;"), "got: {rule}");
        // Niri's softness is CSS's blur radius and its spread is CSS's spread,
        // so the order here is offset-x, offset-y, softness, spread, colour.
        assert!(
            rule.contains("box-shadow: 5px 2px 30px 8px #00000080;"),
            "the shadow should transcribe 1:1 into CSS box-shadow, got: {rule}"
        );
    }

    #[test]
    fn a_negative_shadow_offset_survives_the_transcription() {
        // CSS takes signed offsets, so a shadow cast up and to the left needs
        // no special handling — but it is worth pinning that we don't drop the
        // sign on the way through.
        let appearance = Appearance {
            shadow: Some(Shadow {
                offset: Offset { x: -4, y: -2 },
                ..Shadow::default()
            }),
            ..Appearance::default()
        };
        assert!(
            frame_rule(&launcher_css(Some(&appearance))).contains("box-shadow: -4px -2px "),
            "a negative offset must reach the stylesheet as written"
        );
    }

    #[test]
    fn the_generated_stylesheet_keeps_the_shared_rules() {
        // `BASE_CSS` carries the running-indicator dot and the SearchEntry
        // flattening; both paths need them.
        for css in [
            launcher_css(None),
            launcher_css(Some(&Appearance::default())),
        ] {
            assert!(css.contains(".running-indicator {"), "got: {css}");
            assert!(css.contains("searchentry,"), "got: {css}");
            assert!(css.contains("scrolledwindow > viewport,"), "got: {css}");
        }
    }
}
