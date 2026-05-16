# Launcher UI: search, navigate, activate

## Summary

Add a keyboard-driven launcher window. `lofi-core` gains a `matcher` module (Skim fuzzy search). `lofi-gnome` gains `ui` (window + list + signals) and `launch` (activation via gio) modules. `main.rs` is rewritten to gather entries and hand them to `ui::build`. Matcher gets 6 unit tests; UI/launch are manually verified.

## File-by-file changes

### 1. `app/core/Cargo.toml` — modify

Add to `[dependencies]`: `fuzzy-matcher = "0.3"`.

### 2. `app/core/src/lib.rs` — modify

Insert at the very top:
```rust
pub mod matcher;
pub use matcher::search;
```
Nothing else changes; existing types/tests untouched.

### 3. `app/core/src/matcher.rs` — new

Imports: `fuzzy_matcher::FuzzyMatcher`, `fuzzy_matcher::skim::SkimMatcherV2`, `crate::Entry`.

**Private helper** `fn haystack(entry: &Entry) -> String` — exhaustive `match`. For `Entry::Application(app)` → `format!("{} {}", app.name, app.desktop_id)`. No `_` arm.

**Public** `pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry>`:
- Empty/whitespace query → return all entries in input order (refs).
- Else: tokenize with `query.split_whitespace()`. Build `SkimMatcherV2::default().ignore_case()`. For each entry, compute haystack, fuzzy-match every token; if any token returns `None`, drop the entry; else sum scores into `i64`.
- Sort `sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())))` — score desc, name asc tiebreaker.
- Map sorted tuples → `Vec<&Entry>`.

**Six unit tests** in `#[cfg(test)] mod tests`. Local helper: `fn app(name, desktop_id) -> Entry { Entry::Application(Application { name: name.into(), desktop_id: desktop_id.into(), icon: None }) }`.

1. `empty_query_returns_all_entries_in_input_order` — empty and whitespace-only both return refs in slice order.
2. `single_token_matches_name_case_insensitively` — `"FIRE"` against `["Firefox", "Files", "Chromium"]` → Firefox + Files (in some score order), no Chromium.
3. `single_token_matches_desktop_id` — `"google"` against `("Chrome","com.google.Chrome.desktop")` + decoy → only Chrome.
4. `multi_token_is_order_independent_and_intersects` — `"chr goo"` and `"goo chr"` both match `("Google Chrome", ...)` and not `("Google Earth", ...)` or `("Firefox", ...)`.
5. `score_sort_descending_with_name_tiebreaker` — pin tiebreaker: two entries identical-haystack-shape so scores tie; assert lexicographically smaller name first.
6. `non_matching_query_returns_empty` — `"qqqqq"` against three entries → `Vec::new()`. Also assert `search(&[], "anything")` is empty.

### 4. `app/gnome/src/lib.rs` — modify

Replace contents with:
```rust
pub mod apps;
pub mod ui;
pub mod launch;

pub use apps::{application_directories, gather_applications};
pub use lofi_core::Application;
```

### 5. `app/gnome/src/launch.rs` — new

Imports:
- `use gio_unix::DesktopAppInfo;`
- `use gtk::gio::prelude::*;` (brings `AppInfoExt::launch`)
- `use gtk::prelude::*;` (for `Display`)
- `use lofi_core::Entry;`

```rust
pub fn activate(entry: &Entry)
```

Exhaustive `match` on `Entry::Application(app)`:
1. `let info = match DesktopAppInfo::new(&app.desktop_id) { Some(i) => i, None => { eprintln!("lofi: no DesktopAppInfo for {}", app.desktop_id); return; } };`
2. Build `context` from `gtk::gdk::Display::default()` → `.app_launch_context()`. If gtk4-rs 0.11 returns a `gdk::AppLaunchContext` that doesn't auto-upcast to `gio::AppLaunchContext`, call `.upcast::<gtk::gio::AppLaunchContext>()`. Coder verifies via `cargo check`.
3. `if let Err(e) = info.launch(&[], context.as_ref()) { eprintln!("lofi: launch failed for {}: {e}", app.desktop_id); }`

No `Result` return.

### 6. `app/gnome/src/ui.rs` — new

Constants: `WINDOW_WIDTH = 480`, `WINDOW_HEIGHT = 500`, `ICON_SIZE = 24`.

Imports: `std::cell::RefCell`, `std::rc::Rc`, `adw::prelude::*`, `gtk::prelude::*`, `gtk::glib`, `lofi_core::{Entry, search}`, `crate::launch`. Plus `use gtk::pango;` for ellipsize mode.

**Internal state**:
```rust
struct UiState {
    entries: Vec<Entry>,
    visible: Vec<usize>,  // indices into entries, display order
}
```

**Public entry point**:
```rust
pub fn build(app: &adw::Application, entries: Vec<Entry>)
```

Steps:
1. Build `gtk::SearchEntry` with hexpand + margins.
2. Build `gtk::ListBox` with `selection_mode=Single`, `activate_on_single_click=false`.
3. Wrap list in `gtk::ScrolledWindow` (`hscrollbar=Never`, `vscrollbar=Automatic`, `vexpand=true`).
4. Vertical `gtk::Box` containing search entry + scrolled window.
5. Build window:
   ```rust
   let window = adw::ApplicationWindow::builder()
       .application(app)
       .title("LoFi")
       .default_width(WINDOW_WIDTH)
       .default_height(WINDOW_HEIGHT)
       .resizable(false)
       .decorated(false)
       .modal(true)
       .build();
   window.set_content(Some(&content));
   ```
   Note: `adw::ApplicationWindow::set_content` (NOT `set_child`). Coder verifies.
6. `let state = Rc::new(RefCell::new(UiState { entries, visible: Vec::new() }));`

**Helper `fn populate_list(list_box, state, query)`**:
- Remove all existing rows: `while let Some(row) = list_box.first_child() { list_box.remove(&row); }`
- If query is empty/whitespace: `visible = (0..entries.len()).collect()`. Else: run `search(&state.borrow().entries, query)`, map result refs back to indices via `std::ptr::eq`. Scope the read borrow before writing.
- If `visible.is_empty() && !query.trim().is_empty()`: append one non-selectable `gtk::ListBoxRow` containing a centered `gtk::Label::new(Some("No matches"))` with CSS class `dim-label`; call `row.set_selectable(false)`.
- Else: for each index in `visible`, call `build_row(&entries[idx])` and `list_box.append(&row)`. Select first row.

**Helper `fn build_row(entry: &Entry) -> gtk::ListBoxRow`**:
- Horizontal `gtk::Box` with 8px spacing + reasonable margins.
- Image:
  - `entry.icon()` is `Some(s)` starting with `/` → `Image::from_file(Path::new(s))`
  - `Some(s)` otherwise → `Image::from_icon_name(s)`
  - `None` → `Image::new()` (empty placeholder)
  - In every branch: `image.set_pixel_size(ICON_SIZE)`. NOT `set_icon_size`.
- Name label: `halign=Start`, `hexpand=true`, ellipsize End, single-line, `xalign=0.0`.
- Kind label: `halign=End`, CSS class `dim-label`, text from `kind_to_str(entry.kind())`.
- Wrap in `ListBoxRow::new()` via `row.set_child(Some(&hbox))`.

**Helper `fn kind_to_str(kind: EntryKind) -> &'static str`**: exhaustive `match`. `EntryKind::Application => "Application"`.

**Wire `search_entry.connect_search_changed`**: clone Rc/widgets into the closure; body calls `populate_list(&list_box, &state, &entry.text())`.

**Wire `EventControllerKey` on search_entry**:
- Up/Down: read `list_box.selected_row().and_then(|r| r.index())`. If valid, `list_box.row_at_index(i ± 1)` and `list_box.select_row(Some(&r))`. Do NOT call `grab_focus` — focus stays on the search entry.
- Enter (`Return` or `KP_Enter`): get selected index; look up in `state.borrow().visible[i]`; clone the entry out (drop borrow); call `launch::activate(&entry)`; call `window.close()`.
- Escape: `window.close()`.
- Default: propagate.
- Return type may be `glib::Propagation::{Stop, Proceed}` or `bool`. Coder verifies; the plan uses Propagation names symbolically.
- `search_entry.add_controller(key);`

7. Initial `populate_list(&list_box, &state, "")`.
8. `window.present();`

### 7. `app/gnome/src/main.rs` — replace

```rust
use adw::prelude::*;
use gtk::glib;
use lofi_core::Entry;
use lofi_gnome::{apps, ui};

const APP_ID: &str = "dev.jplein.LoFi";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(on_activate);
    app.run()
}

fn on_activate(app: &adw::Application) {
    let dirs = apps::application_directories();
    let applications = apps::gather_applications(&dirs);
    let entries: Vec<Entry> = applications.into_iter().map(Entry::Application).collect();
    ui::build(app, entries);
}
```

### 8. `app/core/README.md` — modify

In "Current contents", add a subsection for the matcher: signature, behavior summary (empty=passthrough, tokenize, score-sum, sort desc by score / asc by name). Note the haystack is per-variant by exhaustive match (so future variants force an update). Mention the new `fuzzy-matcher` dep.

### 9. `app/gnome/README.md` — modify

Add `ui` and `launch` bullets to Modules. Add a brief "Keyboard" subsection: Up/Down navigate, Enter activates + closes, Escape closes, all other keys go to the search entry. Note that the list is rebuilt on every `search-changed`.

## Implementation order

1. `app/core/Cargo.toml` — add `fuzzy-matcher`.
2. `app/core/src/matcher.rs` — full body + tests.
3. `app/core/src/lib.rs` — declare + re-export. `cargo test -p lofi-core` passes.
4. `app/gnome/src/launch.rs` — full body.
5. `app/gnome/src/ui.rs` — full body.
6. `app/gnome/src/lib.rs` — declare new modules.
7. `app/gnome/src/main.rs` — replace.
8. READMEs.

## Verification (from `app/`)

1. `cargo build --workspace`
2. `cargo test --workspace`
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo fmt --all -- --check`
5. **Manual**: `cargo run -p lofi-gnome --bin lofi`. Window opens; typing filters; Up/Down navigate; Enter launches + closes; Escape closes; empty filter shows "No matches".

## Lint / style notes

- Closures into signals must be `move` (signals require `'static`). Clone Rc/widgets into shadowing bindings before the closure.
- Borrow discipline on `Rc<RefCell<UiState>>`: scope borrows tightly; never overlap `borrow` and `borrow_mut`. The Enter handler reads `visible[i]`, drops borrow, then clones the entry out.
- `ListBoxRow::index()` returns `i32`; cast via `usize::try_from(i).ok()` or guard `i >= 0` to keep clippy quiet.
- All `match`es on `Entry` and `EntryKind` are exhaustive (no `_` arm).
- No `let _ = info.launch(...)`; bind and log the error.

## gtk4-rs 0.11 API spots to verify with `cargo check`

- `adw::ApplicationWindow::set_content` (not `set_child`).
- `SearchEntry::connect_search_changed` (preferred over `connect_changed`).
- `Image::set_pixel_size` (not `set_icon_size`).
- `EventControllerKey::connect_key_pressed` return: `glib::Propagation` vs `bool`.
- `gdk::Display::app_launch_context()`: may or may not require an explicit upcast to `gio::AppLaunchContext` at the `info.launch` call site.
- `gtk::pango::EllipsizeMode::End`.

## Out of scope

MRU/history ordering; Wayland centering; dismiss-on-focus-loss; CSS theming beyond `dim-label`; single-instance hotkey; new Entry variants; tests for `launch::activate`; localization / RTL / accessibility; async gather.
