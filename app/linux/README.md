# app/linux

The Linux implementation of LoFi. One `lofi` binary, two desktops: **GNOME** and **Niri**.

## Stack

- Rust
- GTK4 via [`gtk4-rs`](https://gtk-rs.org/gtk4-rs/)
- libadwaita via [`libadwaita-rs`](https://gtk-rs.org/gtk4-rs/git/docs/libadwaita/) for the launcher window styling
- [`gtk4-layer-shell`](https://docs.rs/gtk4-layer-shell) for presenting the launcher as a `wlr-layer-shell` overlay surface under Niri
- [`gio-unix`](https://docs.rs/gio-unix) for `DesktopAppInfo` (Unix-only, not re-exported from the cross-platform `gtk::gio`)
- [`zbus`](https://docs.rs/zbus) for talking to the LoFi GNOME extension, and to logind, over D-Bus. The blocking proxy (`gen_blocking = true`, `gen_async = false`) is used deliberately: the GTK main thread is synchronous and the gather happens once at startup, so the cost of an async runtime would buy nothing here.
- [`serde_json`](https://docs.rs/serde_json) for Niri's newline-delimited-JSON IPC
- [`toml`](https://docs.rs/toml) for the user's configuration file. Deserialize only — LoFi never writes the file back. Pinned to the same major the gtk4-sys build scripts already pull in through `system-deps`, so it adds no new crate version to the lock file.

This crate is built as both a library (`lofi_linux`) and a binary (`lofi`) so integration tests can link against the library.

## Why one crate and not two

The directory was `app/gnome` when GNOME was the only Linux target. Splitting it into `app/gnome` + `app/niri` when Niri arrived would have duplicated `ui` (the whole launcher window), `apps` (XDG `.desktop` enumeration), `launch`'s application branch, and `main` — none of which knows or cares which compositor is running. What actually differs between the two desktops is a few hundred lines of window/workspace plumbing.

So the crate stayed one crate, the directory was renamed to match what it holds, and the difference was pushed behind a `Backend` trait (`backend/`) chosen at **run time**. A build-time Cargo feature was rejected for the same reason: it would mean two binaries, two Nix packages, and a home-manager option the user has to keep in sync with the session they actually log into — for no gain, since linking both costs nothing measurable.

## Modules

- `apps` — enumerates installed applications by parsing `.desktop` files via `gio_unix::DesktopAppInfo`. Desktop-agnostic: this is XDG, not window-system.
  - `application_directories()` returns the XDG-driven search list: `$XDG_DATA_HOME` (falling back to `$HOME/.local/share`), then each entry of `$XDG_DATA_DIRS` (falling back to `/usr/local/share:/usr/share`), each with `applications` appended.
  - `gather_applications(dirs)` reads the supplied directories, skips missing ones silently, and returns a `Vec<lofi_core::Application>`. Entries that fail `should_show()` (per the freedesktop spec) are filtered out. Non-recursive. Each returned `Application` includes `icon: Option<String>` populated from `DesktopAppInfo::icon()` via `gio::IconExt::to_string` — the freedesktop serializer (`g_icon_to_string`) for the icon GObject. The value is an icon **identifier**, not bytes: rendering is deferred to the GTK image widget at draw time, where the icon theme, scale, and target size are known. `gather_applications` guarantees that every `Application::desktop_id` is canonical — always ends in `.desktop`. The integration test pins this invariant. Canonicalization matters because `desktop_id` is the payload of `EntryRef::Application` (see `lofi-core`) and therefore the stable history/MRU key; a bare stem would break round-tripping with previously persisted references. Results are deduped by canonical `desktop_id` with first-directory-wins semantics — this is the XDG shadowing convention, and the dir order from `application_directories()` already produces the right precedence (`$XDG_DATA_HOME` shadows `$XDG_DATA_DIRS`), so the dedup belongs here rather than at a caller; a user installing Ghostty via both the Nix system profile and `~/.local/share/applications` would not expect it to appear twice in the launcher.
- `backend` — the desktop seam. See [Backends](#backends) below.
- `config` — the user's configuration file. See [Configuration](#configuration) below.
- `launch` — `launch::activate(&dyn Backend, &Entry)` is the single dispatch point for "the user pressed Enter on this row". An exhaustive `match` routes entries:
  - `Entry::Application(app)` branches on `app.recent_window_id`. When `Some(id)`, the app is currently running and `activate` calls `backend.focus_window(id)` — raising the most-recently-used window of the app, mirroring the GNOME dock's "click a running app's icon = raise its window" behaviour. When `None`, it falls back to a `gio_unix::DesktopAppInfo::new` lookup + `info.launch(&[], context.as_ref())` (the `gdk::Display::default().app_launch_context()` carries the launching display so the new app starts on the right monitor). This is the one branch that doesn't go through the backend: `.desktop` activation is XDG, identical on both desktops. We deliberately do **not** fall back from focus to launch when `focus_window` fails: the gather-vs-click race is real but rare, and a phantom second instance would be more surprising than a silent no-op.
  - `Entry::Window(w)` → `backend.focus_window(w.id)`.
  - `Entry::Workspace(w)` → `backend.activate_workspace(w.index)`.
  - `Entry::Command(cmd)` → `backend.run_command(cmd)`.
  - `Entry::WorkspaceCommand(wc)` → `backend.run_workspace_command(wc)`.
  - `Entry::PowerCommand(c)` → `backend.run_power_command(c.kind)`.

  Errors at any branch are logged to stderr and swallowed: there's no useful recovery from "the desktop file vanished between gather and click", "the window id no longer resolves", or "the workspace was removed between gather and click" at the UI layer.
- `ui` — the launcher window. Public entry point `ui::build(app, entries, mru_store, mru_index, backend, appearance)` constructs an `adw::ApplicationWindow` containing a `SearchEntry` over a scrolled `ListBox` and presents it. Internally holds the full gathered set in an `Rc<RefCell<UiState>>` alongside a `visible: Vec<usize>` of indices into that set and a `mru_position: HashMap<EntryRef, usize>` (rank 0 = most recent) built from `mru_index` at construction time. On every `changed` (keystroke) the list is fully torn down (`while let Some(child) = list_box.first_child()`) and rebuilt — simpler than diffing and fast enough at the scale of an application gather. The handler deliberately uses `changed` rather than GtkSearchEntry's debounced `search-changed`, which would otherwise delay the rebuild ~150ms after the last keypress. `populate_list` does **not** do its own filtering or sorting — it delegates the whole thing to `lofi_core::rank(&entries, query, &mru_position)`, the single shared ranking implementation in `app/core` (see `app/core/README.md`'s `ranking::rank` section). `rank` handles filtering (intersection semantics), the two-tier MRU/score order, the in-MRU prefix sub-ordering, and the empty-query passthrough, and returns indices into the gathered set in display order. The Linux layer's only ranking responsibility is to gather the entries and supply the MRU recency map; the macOS FFI path (`recompute_filter`) calls the same function, so the two platforms produce identical order. If the result is empty the list shows a single non-selectable "No matches" row.

  The backend reaches `ui` for exactly two reasons: it is captured by the Enter and click closures so `launch::activate` can dispatch, and it is consulted once — `backend.uses_layer_shell()` — for how to present the window (see [Window presentation](#window-presentation)). Everything else in this module is desktop-agnostic.

  Ordering is two-tier (computed in `lofi_core::rank`): MRU recency is the **dominant** signal — the user's known apps are held ahead of never-launched matches, which sort by descending fuzzy score. Within the MRU tier a *prefix* sub-signal floats word-prefix matches above non-prefix ones, but only *within* that tier — a never-launched app can never jump above a known one just because the query word-prefixes its name. The practical effect: a recently-used row stays predictably near the top while the user types, while never-launched rows can reorder as the query narrows and the scores shift. See `app/core/README.md`'s `ranking::rank` section for the exact algorithm (recency rank, prefix bucket, score) and the empty-query passthrough.

  Each row's icon column is a vertical `gtk::Box` containing the `gtk::Image` plus a small `gtk::Box` for the running-indicator dot (6x6, circular via `border-radius: 9999px`, coloured `alpha(@theme_fg_color, 0.8)` so it adapts to light/dark themes). The dot widget is always appended at its full 6x6 size so every row's icon column has identical height; the `.running-indicator` CSS class is added only for running Applications, and non-running entries get an unstyled (transparent) box that occupies the same space. This is what keeps rows visually aligned — `set_visible(false)` would defeat it, since GTK4 drops invisible widgets from layout entirely and rows would shift vertically between running and non-running entries. The launcher's CSS (`LAUNCHER_CSS`) covers the surface colour, the running-indicator dot, and the top SearchEntry, which is stripped of its default rounded border, focus ring, and tinted fill (`background: transparent; box-shadow: none; border: none; outline: none` on both `searchentry` and its inner `text` node) so the input blends into the window background instead of looking like a separate inset control. The window itself is painted `@popover_bg_color` / `@popover_fg_color` — the libadwaita **menu** surface rather than the window (`@window_bg_color`) or content-view (`@view_bg_color`) one — because the launcher is conceptually a menu, not a document window; naming the libadwaita colours rather than literals is what makes it follow the light/dark scheme. That only reads as one surface if nothing underneath repaints: GTK gives a bare `list`, and the viewport and scroller wrapping it, the VIEW background, which would punch a panel of a visibly different tone through the popover surface, so `scrolledwindow`, `scrolledwindow > viewport` and `list` are all forced transparent. The viewport is named even though it paints nothing under Adwaita today — it costs no pixel, and it is what stops a theme that *does* give it a background from reintroducing the panel one level down. CSS is registered once per process via `install_styles()`, gated by an `OnceLock<()>` latch (`STYLES_INSTALLED`) because `build()` runs on every `connect_activate` firing and re-registering the same provider would stack identical priority entries. `install_styles` is a silent no-op when there is no default `gdk::Display` (headless tests, broken environment); the dot falls back to whatever GTK renders for an unstyled empty `gtk::Box` in that case and the SearchEntry keeps its default Adwaita styling.

### Window presentation

On GNOME the launcher is an ordinary toplevel. GNOME floats every window, so nothing special is needed.

On Niri an ordinary toplevel would be **tiled into the scrolling layout** — the launcher would shove the user's columns aside every time it opened, and would appear in its own window list. So under Niri `ui::configure_layer_shell` turns the window into a `wlr-layer-shell` surface before `present()`:

- **Overlay layer**, not `Top`. Niri renders a focused fullscreen window over the `Top` layer, so `Top` would leave the launcher invisible exactly when a video or a game is fullscreen — a moment you very much want a launcher.
- **Exclusive keyboard**. `OnDemand` would require a click before typing reached the search field, which defeats the point.
- **No anchors**. A layer surface anchored to no edge is centred by the compositor at the window's own requested size, which is the placement we want without fetching output geometry to compute a margin.
- **Namespace `lofi`**, which is what `niri msg layers` shows and what a user would match in a `layer-rule`.

`init_layer_shell` must run before the window is realised, so the call sits immediately before `present()`. It is guarded by `gtk4_layer_shell::is_supported()`; that guard should be unreachable (only the Niri backend asks, and Niri implements the protocol) but the failure mode without it is a GTK-level abort rather than a launcher that merely comes up as an ordinary window.

A useful side effect: a layer surface is not a toplevel, so LoFi does not appear in Niri's own `Windows` list at all. It cannot turn up as a row in its own window list, and it cannot be picked as the target of its own window commands. The `LOFI_DESKTOP_ID` filter in the Niri backend's `target_window` is therefore belt-and-braces, kept for the layer-shell-unavailable fallback path.

### Decorations, and who draws them

The two presentation paths differ in one more way, and it is the reason the `config` module exists.

On the **toplevel** path the window keeps its client-side decorations — `ui::build` deliberately does not call `decorated(false)` — so GTK draws the window shadow and clips the rounded corners, and `AdwApplicationWindow` gives us no titlebar to suppress. Nothing in the config file touches this path.

On the **layer-shell** path there are no decorations at all, and neither GTK nor Niri can supply them:

| | who can draw it on a layer surface |
|---|---|
| Drop shadow | GTK (with a margin) **or** Niri, via `layer-rule { shadow { … } }` |
| Corner rounding of the surface | LoFi only |
| Border | LoFi only |

- `libgtk4-layer-shell` calls `gtk_window_set_decorated(FALSE)` on the window it converts, so there is no CSD decoration node. Left alone, the launcher renders as a bare rectangle: measured against a screenshot, the surface is exactly `WINDOW_WIDTH` × `WINDOW_HEIGHT` with no shadow margin, and a hard 90° step from wallpaper to surface at every corner.
- Niri's layer rules accept `shadow` and `geometry-corner-radius`, but `geometry-corner-radius` on a layer rule only shapes **the shadow** — `clip-to-geometry` is window-rule-only, so it cannot round the surface. `border` and `focus-ring` are window-rule-only too, and Niri's parser rejects them outright inside a `layer-rule`.

So LoFi always draws its own radius and border, and the shadow is either side's to draw. `ui::launcher_css(appearance)` generates the stylesheet for whichever arrangement the config asks for, and `ui::build` wraps `content` in a frame `gtk::Box` carrying `.lofi-frame`:

- The surface colour moves off the `window` node and onto `.lofi-frame`; `window` goes transparent, because a window still painting its own corners would show an opaque square underneath the frame's rounded one.
- The frame gets `Overflow::Hidden`, which in GTK4 pushes a **rounded** clip derived from the node's CSS padding box — that is what stops a selected row's highlight from squaring off a corner it reaches.
- A `border` declaration is emitted only when `border-width` is non-zero, rather than as `0px solid`, so the default config adds no declaration at all.

**When the config names a shadow**, LoFi draws it as a CSS `box-shadow` on the frame, and the frame takes a transparent margin of `softness + spread + max(|offset.x|, |offset.y|)` on all four sides with the window's default size grown by twice that. The margin is symmetric on purpose: the launcher's layer surface is anchored to no edge, so it is the **surface** the compositor centres, and per-edge margins would shift the visible window off-centre by half the difference. The margin is also capped (`config::MAX_SHADOW_MARGIN`) because it grows the surface itself — every other setting is clamped by GTK on its own, but a fat-fingered `softness` here would ask the compositor for a surface bigger than the screen.

**When it doesn't** — the default — no margin is added and the surface stays exactly the visible rectangle. That is precisely the shape Niri's own `layer-rule { shadow { on } }` needs: Niri draws a layer surface's shadow around the whole buffer, invisible margins included, because a layer surface has no `xdg_surface.set_window_geometry` to declare its visual bounds with. Niri's own docs warn about this ("you'll need to configure layer-shell clients to remove their own margins or shadows"). So the two are mutually exclusive by construction, and leaving the shadow block out is how a user opts into the compositor drawing it.

### Keyboard

- **Up / Down** — move the selection in the list. Focus stays on the search entry, so typing continues to filter without an extra Tab.
- **Enter** (Return or KP_Enter) — `launch::activate` the selected entry and close the window. A no-op when the "No matches" row is the only thing on screen, because that row is not selectable.
- **Escape** — close the window without launching.
- Everything else propagates to the search entry, so normal text editing keeps working.

The list is rebuilt from scratch on every keystroke. There is no incremental diff and no debounce; both are unnecessary at application-gather scale. The handler is wired to the `changed` signal (from `GtkEditable`), **not** GtkSearchEntry's own `search-changed` — the latter is debounced by ~150ms inside GTK, which the user perceives as a lag between typing and the list updating. `changed` fires synchronously per keystroke. (`GtkSearchEntry:search-delay = 0` would be equivalent but needs the gtk4-rs `v4_8` feature, and this crate targets the unfeatured GTK baseline.)

Integration tests live in `tests/` and build their own `.desktop` fixtures inside a `tempfile::tempdir()`. The gatherer takes directories as a parameter, so tests never mutate process environment variables. `ui` and `launch` are exercised manually — they need a Wayland session and a running compositor to be meaningful.

## Configuration

`~/.config/lofi/config.toml` (`$XDG_CONFIG_HOME/lofi/config.toml` when that is set). Every key is optional and the whole file is optional; an absent file is the common case and yields the defaults silently.

```toml
lock-command = "swaylock -f -c 000000"

[appearance]
corner-radius = 12          # default 12
border-width  = 3           # default 0 — no border
border-color  = "#e0e0e0"   # default @borders

[appearance.shadow]         # omit for no LoFi-drawn shadow
softness = 30               # default 30
spread   = 8                # default 5
offset   = { x = 5, y = 5 } # default x=0 y=5
color    = "#00000080"      # default #0007
```

### Why a file, and why not read Niri's

The launcher has to be *told* what the user's Niri looks like, because on the layer-shell path it has to draw decorations nobody else can (see [Decorations, and who draws them](#decorations-and-who-draws-them)). It is told rather than deduced: LoFi deliberately does **not** parse `config.kdl`. That file is the compositor's, its schema is Niri's to change, and the values LoFi would want (`layout.border`, a window rule's `geometry-corner-radius`) are per-window-rule anyway — there is no single correct answer to read out of it. A handful of numbers written down once is simpler and predictable, which is the launcher's stated goal.

TOML because `serde` was already a dependency and `toml` was already in the lock file via `system-deps`, so the format costs one line. KDL would let the `[appearance.shadow]` block be a literal copy-paste out of `config.kdl`, but would add a parser this crate otherwise has no use for.

The shadow **key names and defaults are Niri's**, though, and that is not cosmetic: Niri defines its shadow parameters by reference to CSS ("`softness` … same as CSS box-shadow blur radius", "`spread` … same as CSS box-shadow spread", "`offset` … same as CSS box-shadow offset"), and LoFi renders the shadow as a CSS `box-shadow`. The translation is therefore exact, the numbers copy straight across, and a bare `[appearance.shadow]` with no keys reproduces Niri's stock shadow.

### Scope and error policy

`[appearance]` is honoured **only** on the layer-shell path. On an ordinary toplevel GTK's client-side decorations already supply the shadow and the rounding, and drawing a second rounded, bordered frame inside one would double up — so on GNOME the generated stylesheet is byte-identical to what it was before the config file existed, default radius included.

Errors follow the crate's usual policy: log to stderr and degrade. A missing file is silent; an unreadable or malformed one logs and leaves the launcher on defaults. A launcher that refused to open over a typo in a cosmetic setting would be a much worse outcome than one that opens looking plain.

Two deliberate strictnesses inside that:

- **`deny_unknown_fields`.** A misspelled `border-colour` is an error naming the key, not a silently ignored line — the alternative is a user staring at an unchanged window with nothing to go on. The cost is that one typo drops the whole file back to defaults, which is at least visible.
- **Colours are validated, not passed through.** `config::Color` accepts a hex colour (`#0007`, `#e0e0e0`, `#00000080`), a GTK named colour (`@borders`, `@accent_color`), or a bare CSS keyword (`black`), and nothing else. This is a security property rather than a style preference: the value is interpolated into a GTK stylesheet, so a raw pass-through would let a config file close the declaration and open its own (`#fff; } * { background-image: url(…)`). What the three accepted shapes have in common is no whitespace and no `;`, `}`, `(`, or `/`. An unknown but well-shaped keyword is left for GTK to reject on its own terms.

### Testing

`config::parse(text)` is pure and separate from `config::load_from(path)`, for the same reason `apps::gather_applications` takes its directories as a parameter and `backend::detect` takes its environment values: the interesting half is unit-testable without touching the filesystem. `ui::launcher_css(appearance)` is split out of `ui::install_styles` on the same principle — the generated declarations are asserted on without needing a `gdk::Display`.

One test (`the_home_manager_modules_output_shape_parses`) pins the shape the home-manager module actually emits. `pkgs.formats.toml` renders a nested table as its own `[appearance.shadow.offset]` header rather than as an inline table, so the generated file does not look like the hand-written one above; both are the same TOML data model, and nothing else would catch the module and the parser drifting apart.

## Backends

`backend::Backend` is the trait; `backend::create(&config)` picks an implementation and returns it as an `Rc<dyn Backend>` that lives for the whole launcher invocation. `config` is consulted for exactly one thing today — the Niri backend's `lock-command`, which `NiriBackend` copies out because it outlives the gather step that owns the `Config`. The GNOME backend ignores it: its Lock goes through `org.gnome.ScreenSaver`, which needs no locker to be named.

### Detection

`backend::detect(niri_socket, xdg_current_desktop)` is a pure function over two environment values (passed in rather than read, so it is unit-tested without mutating process state — the same reason `apps::gather_applications` takes its directories as a parameter):

1. `$NIRI_SOCKET` set and non-empty → **Niri**. This is the primary signal because it is the thing the backend actually needs; Niri exports it into every process it spawns, and its presence means there is a socket to talk to.
2. Otherwise, `$XDG_CURRENT_DESKTOP` with a `niri` component (case-insensitive, colon-separated per the spec) → **Niri**. This only matters when the launcher was started outside the compositor's own environment — a systemd user unit that didn't import it, an `ssh` shell. We would rather pick Niri and log a missing-socket error than silently pick GNOME and wait on a D-Bus name that will never be owned.
3. Otherwise → **GNOME**. The historical default, so existing installs keep working with no configuration.

### The trait

The gather half is called once at startup, before the window is shown; the act half is called once, from a UI closure, immediately before the window closes. Nothing in between re-reads — the launcher is a short-lived process and the snapshot taken at startup is what the user is looking at when they press Enter.

```rust
fn gather_windows(&self) -> Vec<Window>;
fn gather_workspaces(&self) -> Vec<Workspace>;
fn gather_commands(&self, windows: &[Window]) -> Vec<Command>;
fn gather_workspace_commands(&self, windows: &[Window], workspaces: &[Workspace]) -> Vec<WorkspaceCommand>;
fn gather_power_commands(&self) -> Vec<PowerCommand>;
fn gather_summon_commands(&self, windows: &[Window]) -> Vec<SummonWindow>;

fn focus_window(&self, id: u64);
fn activate_workspace(&self, index: i32);
fn run_command(&self, command: &Command);
fn run_workspace_command(&self, command: &WorkspaceCommand);
fn run_power_command(&self, kind: PowerCommandKind);
fn run_summon_command(&self, summon: &SummonWindow);

fn uses_layer_shell(&self) -> bool;
```

The act half is deliberately **coarse**. `run_command` takes a whole `Command` rather than exposing `move_resize_window` / `minimize_window` / … as trait methods, because the two desktops disagree about what a window action *is*: GNOME computes a rectangle with `lofi_core::compute_geometry` and sends one `MoveResizeWindow`, while Niri has no free-form geometry for tiled windows and sends a proportional column-width action instead. A per-primitive trait would force one desktop's vocabulary onto the other. Same for `run_workspace_command`, which GNOME performs as two calls (move, then switch) and Niri as one.

`gather_windows` must return **MRU order**, most-recently-focused first. That contract is load-bearing in three places: `main`'s app-to-recent-window combine step takes the first occurrence of each `app_desktop_id`, and both command gatherers take the first non-LoFi entry as their target window.

`Workspace::index` must be **0-based and dense** over the returned slice. It is the payload of `EntryRef::Workspace` and therefore a persistent MRU key, and it is what `activate_workspace` and `run_workspace_command` are handed back.

Every method follows the same error policy the code grew out of: log to stderr and degrade — an empty `Vec` for a gather, a silent no-op for an action. Nothing panics, unwraps, or expects. A launcher that refuses to open because the compositor didn't answer is worse than one that opens with the application list alone.

`backend::LOFI_DESKTOP_ID` (`"dev.jplein.LoFi.desktop"`) is shared rather than duplicated per backend: it is one fact about *this binary*, not about either desktop, and must stay in lockstep with `APP_ID` in `main.rs` and with the installed `.desktop` file's name.

## The GNOME backend

`backend/gnome/` — a blocking D-Bus client for the LoFi GNOME Shell extension (`extension/gnome/`), which publishes `dev.jplein.LoFi.Shell.WindowManager` on the session bus. `GnomeBackend` itself is stateless: every call opens its own short-lived session-bus connection, so there is nothing to cache between the gather pass and the activation call.

The extension is required because Wayland clients can't enumerate or manipulate other apps' windows directly, and Mutter doesn't implement `wlr-foreign-toplevel-management`. `org.gnome.Shell.Introspect` exists and is read-only, but its schema is too narrow (no workspace assignment, no per-window geometry, no monitor scale) to drive the launcher, so the extension publishes its own listing surface rather than the Rust side splitting reads across two D-Bus endpoints.

- `backend/gnome/windows.rs` — `#[zbus::proxy]` blocking client for the interface, plus free functions wrapping each method:
  - `gather_windows()` calls **`ListWindowsMRU`** (not `ListWindows`) and maps each returned dict into a `lofi_core::Window`. The MRU-ordered result is what satisfies the trait's ordering contract. Empty `app_name` / `icon` / `app_desktop_id` strings on the wire become `None`; see `app/core/README.md`'s `Window` subsection for why. On any `zbus::Error` it logs via `eprintln!` and returns an empty `Vec`.
  - `focus_window(id)` calls `FocusWindow`, which raises the window **and** switches the workspace if needed (the extension implements it via `meta_window.activate(time)`).
  - `move_window_to_workspace(id, target_index)` — by-id wrapper around `MoveWindowToWorkspace`. The extension unsticks an on-all-workspaces window first and (under dynamic workspaces) appends a workspace when the target is one past the end; LoFi only ever passes indices of already-open workspaces, so neither path fires. The `snake_case` name round-trips cleanly through heck to the wire name `MoveWindowToWorkspace`, so no explicit `#[zbus(name = ...)]` is needed (unlike `list_windows_mru`).
  - `minimize_window(id)`, `toggle_maximize_window(id)`, `toggle_fullscreen_window(id)` — by-id wrappers around the corresponding extension methods. Toggle state is resolved on the extension side because Mutter holds the live state and a Rust-side capture-then-act would race against an external change.
  - `move_resize_window(id, x, y, w, h)` — by-id wrapper around `MoveResizeWindow`, given the rectangle produced by `lofi_core::compute_geometry`. The extension unmaximizes / unfullscreens before applying the move (see `extension/gnome/README.md`).
  - `get_window_work_area(id) -> Option<WorkArea>` and `get_window_frame(id) -> Option<(i32, i32, i32, i32)>` — by-id wrappers around `GetWindowWorkArea` and `GetWindowFrame`, used at gather time to bake the work area and current frame into every `Command`. Return `None` on any D-Bus failure so the caller drops the command set entirely rather than running with junk geometry.

  Every wrapper uses the same `eprintln!`-and-degrade policy: a fresh blocking session connection per call, no `unwrap`/`expect`, log on failure and return an empty Vec / unit / `None` depending on the function's success type. The proxy trait declares **both** `list_windows` and `list_windows_mru` for completeness, but only the MRU path is consumed. `list_windows_mru` carries an explicit `#[zbus(name = "ListWindowsMRU")]`: zbus uses `heck` to map Rust `snake_case` method names to PascalCase wire names, and heck would otherwise produce `ListWindowsMru` (treating `MRU` as a regular word), which the extension does not export.

  Reusing a single connection across calls is a deliberate non-goal until profiling shows it matters.
- `backend/gnome/workspaces.rs` — `gather_workspaces()` calls `ListWorkspaces`; `activate_workspace(index)` calls `ActivateWorkspace`. The extension also emits `active` and `n_windows` per workspace dict; zvariant ignores dict keys not declared on the target struct, so they are silently dropped on decode. The proxy trait is declared **independently** from `WindowManager` in `windows.rs` even though both target the same D-Bus interface, service, and path: sharing it would couple the two modules for no win — the wire surface happens to be one object, but window listing/focus and workspace listing/switch are otherwise unrelated concerns.
- `backend/gnome/commands.rs` — builds the two command sets.
  - `gather_commands(windows)` takes the first window whose `app_desktop_id` isn't `LOFI_DESKTOP_ID` as the target, then reads `get_window_work_area(id)` and `get_window_frame(id)`. Returns an empty `Vec` if no non-LoFi window is open or either read fails — matching the original window-commands set's `if (!window) return false` guard. When a target is found it builds fourteen `Command` entries (one per kind in `ALL_KINDS`) sharing the same `target_window_id`, `work_area`, and `current_frame`.

    Targeting by id rather than by "active window" is what makes the activation path race-free. By the time the user presses Enter, LoFi itself is the focused window, so any `*ActiveWindow` D-Bus method invoked from inside the launcher would act on LoFi's own window. Capturing the previously-focused user window's id at gather time means the right window is operated on regardless of focus state at activation — and is also why the extension no longer exposes any `*ActiveWindow` action methods (no Rust caller could use them safely; see `extension/gnome/README.md`). Capturing `work_area` and `current_frame` up front in the same pass keeps `lofi_core::compute_geometry` a pure function over its inputs, so the geometry math is unit-testable in `lofi-core` without touching D-Bus.

    `windows` is passed in rather than re-listed here so this function and `gather_workspace_commands` read the same snapshot — their target picks only agree if they do.
  - `gather_workspace_commands(windows, workspaces)` builds the **dynamic** workspace-move command set: one "Move to workspace N" per open workspace, plus boundary-guarded "Move to previous/next workspace" rows. It makes **no** D-Bus call — it picks the same target as `gather_commands` and reads the destination data it needs straight off the `Window` struct (the target's `id` and its current `workspace` index). Everything else — the per-workspace labelling, the 1-based display numbers, and the first/last boundary logic — lives in the pure `lofi_core::build_workspace_commands`.

    No extra round-trip is needed at **gather** time because, unlike the geometry commands (which need a per-window work area and frame), a workspace move needs only the destination index — pure arithmetic over the workspace list the launcher already has. The relative prev/next destinations are resolved here at gather time (`current ∓ 1`) so the **activation** path needs no reads, just the two writes.
- `backend/gnome/power.rs` — Lock via `org.gnome.ScreenSaver.Lock`; Log Out / Restart / Shutdown via `org.gnome.SessionManager`'s `Logout(0)` / `Reboot()` / `Shutdown()`; Suspend via logind (`backend::logind::suspend`). Routing the first three through SessionManager rather than logind directly is deliberate: those methods raise GNOME's standard confirmation dialog, matching the system-menu UX and protecting against an accidental hotkey hit — a power command is far heavier than the average launcher entry. `Logout(0)` is the with-confirmation mode (1 = no confirmation, 2 = force). The module uses lower-level `Proxy::new` + `call_method` rather than generated `#[zbus::proxy]` traits: each call is one line of business logic, and most of those traits would be one-method wrappers we never reuse.

  **No extension changes** were needed for power: `org.gnome.ScreenSaver`, `org.gnome.SessionManager`, and `org.freedesktop.login1` already exist in any GNOME session, so the LoFi extension stays out of that path entirely.

`GnomeBackend::run_command` dispatches the three state-toggle kinds to their by-id methods and runs the eleven geometry kinds through `compute_geometry` into a single `MoveResizeWindow`. `run_workspace_command` is two sequential blocking calls — move, then switch — so the user *follows* the window they just moved rather than being left behind on the source workspace. Both block, so the move lands before the switch, and each logs-and-degrades independently. Neither dispatch branches on anything the gather didn't already resolve.

## The Niri backend

`backend/niri/` — a client for the compositor's own IPC socket. Niri needs no LoFi-side extension: it ships a complete window/workspace IPC surface in the compositor itself, reachable at `$NIRI_SOCKET`.

- `backend/niri/ipc.rs` — the wire protocol. Connect to the unix socket, write one JSON request on a single line, shut down the write half, read one JSON line back; the reply is `{"Ok": <response>}` or `{"Err": "<message>"}`. A fresh connection per call, for the same reason the GNOME backend opens a fresh D-Bus connection per call. Every request carries a 1-second read/write timeout — not a latency budget (the socket answers in well under a millisecond) but a liveness guard, so a compositor wedged mid-frame doesn't wedge the launcher with it.
- `backend/niri/appid.rs` — resolves a Wayland `app_id` to a desktop entry. See [app_id resolution](#app_id-resolution).
- `backend/niri/power.rs` — power commands. See [Power under Niri](#power-under-niri).

### Why hand-written wire types

We speak the protocol directly rather than depending on the `niri-ipc` crate or shelling out to `niri msg --json`:

- `niri-ipc` is explicitly **not** semver-stable — it tracks the compositor's own version — so pinning it would couple this crate's `Cargo.lock` to whichever Niri the user happens to be running.
- `niri msg` would fork a process per call, and its non-JSON output is documented as explicitly unstable.
- The **JSON** is documented as stable: existing fields and enum variants are not renamed or removed, and new ones may be added.

So the narrowest coupling of the three is hand-written structs against the stable JSON. Every deserialized struct declares only the fields LoFi reads and lets serde ignore the rest, which is exactly the "handle unknown fields gracefully" the Niri IPC docs ask of clients; optional fields are `Option` + `#[serde(default)]` so a *missing* field degrades instead of failing the whole parse.

The request side is pinned by a unit test that asserts each `Request` serializes to a byte-exact string. Those strings were captured from the real client — point `NIRI_SOCKET` at a listening socket and run the equivalent `niri msg action …`, which prints the JSON it would have sent. That is the canonical way to confirm a shape, and the way to re-confirm one if a future Niri changes it.

### MRU ordering

Niri's `Windows` response is in **unspecified order**, so the backend sorts it. The key is `(is_focused, focus_timestamp.secs, focus_timestamp.nanos)`, descending.

`is_focused` is folded in ahead of the timestamp because Niri commits a window to its recent-windows list on a debounce (750 ms by default), so the currently-focused window's `focus_timestamp` can still be the *previous* window's. Without the tiebreak, opening the launcher inside that window would target the wrong window. A window that has never been focused sorts to the very end rather than the front, so a freshly-mapped window can't hijack the command set's target.

(In practice, once the launcher's overlay holds keyboard focus no window reports `is_focused` at all — see below — so the timestamp is what actually orders the list. The tiebreak covers the layer-shell-unavailable fallback path.)

### Workspaces: positional index, Niri id

Niri workspace ids are **not durable**. Under dynamic workspaces the trailing empty workspace on each output is destroyed and recreated with a fresh id as focus moves away from it, so a persisted `EntryRef::Workspace` keyed on a Niri id would rot within a single session.

So the backend keeps a table mapping its own 0-based **position** to Niri's id, and hands positions out as `Workspace::index`. Positions are assigned over the table sorted by output connector name, then Niri's per-output `idx`, then id. Sorting by output name is what makes the order *stable*: Niri's response isn't ordered, and an unstable order would mean a persisted "workspace 2" MRU row pointing at a different workspace on the next launch — exactly the unpredictability LoFi exists to avoid.

The table is fetched once and cached for the invocation. It has to be shared rather than rebuilt per call because three separate things need the *same* mapping: `gather_workspaces` (which hands out the positions), `gather_windows` (which translates each window's `workspace_id` into one), and the two activation paths (which translate back). Rebuilding between them would let a workspace created or destroyed mid-gather shift every index. Each row also caches its output's connector name, so `work_area_for` gets from window to output without a second `Workspaces` request.

Activation reads the **cached** id rather than re-resolving, and that is the point. Re-fetching would resolve the position against whatever the table looks like *now* and could act on a workspace the user never saw; using the cached id means a workspace that has since been destroyed degrades to a no-op instead. Same trade the rest of the launcher makes — a silent no-op beats a surprising action. In practice the window is vanishingly small: LoFi's overlay holds keyboard focus for its whole lifetime, so the user cannot be moving around the compositor while it is open.

Workspace **labels** are built from Niri's own 1-based per-output `idx`, not from the positional index: a named workspace keeps its name verbatim, an unnamed one becomes `Workspace N`, and the output connector is appended only when more than one output is in play. The two numberings agree on a single-monitor session; where they diverge, the number the user's own keybinds use is the one worth showing. The positional index stays internal — a join key, never displayed.

### app_id resolution

GNOME Shell's `WindowTracker` already knows which `Shell.App` owns a window, so the extension hands LoFi a canonical `.desktop` id, a display name, and an icon per window. Niri has no equivalent: its IPC reports the raw `app_id` the client set (or, for XWayland clients, the WM_CLASS) and nothing else.

`appid::AppIdResolver` fills the gap:

1. `<app_id>.desktop` verbatim — the freedesktop convention, and it covers the overwhelming majority of native Wayland clients, both reverse-DNS ids (`com.mitchellh.ghostty`) and short ones (`google-chrome`).
2. `<app_id>.desktop` lowercased — some toolkits capitalise the id (`Alacritty`, `Emacs`) where the desktop file is lowercase.
3. A `StartupWMClass` match — the freedesktop-blessed escape hatch for exactly this problem, and what covers XWayland clients whose WM_CLASS bears no resemblance to their desktop file id.

Step 3 needs an index over every installed desktop entry, so it is built **lazily and only once**: steps 1 and 2 answer almost every window, and paying for a second full scan of the desktop-file tree on every launch — when `apps::gather_applications` has already done one — would be a real cost against the "launches instantly" goal for a case that rarely fires. The index walks the directories directly rather than asking GIO for every installed `AppInfo`, so it uses the same directory list, the same non-recursive walk, the same `should_show()` filter, and the same first-directory-wins shadowing as `gather_applications` — a window resolved here must name a desktop id the application list actually contains, or the running-indicator join in `main` would silently miss.

This matters for the window rows' icons and app names, the running-indicator dot, and the focus-instead-of-launch branch in `launch::activate`. All three degrade gracefully on a miss: the window still lists and still focuses, it just shows the raw `app_id` in place of an app name and carries no icon.

### Window commands under Niri

Niri is a **scrolling tiler**, so there is no free-form window geometry: a tiled window's position is a consequence of its place in the scroll order and its height is the column's. The eleven `compute_geometry` kinds therefore have no meaning here and are not emitted, and `Minimize` isn't either — Niri has no minimized state at all, so there is nowhere for a window to go.

What the backend emits instead, in list order:

| Row | `CommandKind` | Niri action |
| --- | --- | --- |
| Center | `Center` | `CenterWindow { id }` |
| Width third | `WidthThird` | `SetWindowWidth { id, SetProportion(33.3) }` |
| Width half | `WidthHalf` | `SetWindowWidth { id, SetProportion(50.0) }` |
| Width two-thirds | `WidthTwoThirds` | `SetWindowWidth { id, SetProportion(66.7) }` |
| Maximize column | `MaximizeColumn` | `SetWindowWidth { id, SetProportion(100.0) }` |
| Expand column | `ExpandColumn` | `ExpandColumnToAvailableWidth {}` |
| Toggle floating | `ToggleFloating` | `ToggleWindowFloating { id }` |
| Toggle maximize | `ToggleMaximize` | `MaximizeWindowToEdges { id }` |
| Toggle fullscreen | `ToggleFullscreen` | `FullscreenWindow { id }` |
| Close window | `Close` | `CloseWindow { id }` |

"Maximize column" is `SetWindowWidth` at 100%, not Niri's `MaximizeColumn` action, because Niri's own documentation defines maximize-column as equivalent to a column width of 100% — and `SetWindowWidth` takes a window id where `MaximizeColumn` would act on whatever is focused. That by-id preference is the same principle as the GNOME backend's: the launcher decides its target at gather time and names it explicitly.

`ExpandColumn` is an exception, because Niri exposes no by-id form of it (the other is `MoveColumnToIndex`; see [Summon window under Niri](#summon-window-under-niri)). It acts on Niri's **layout** active window, which is not the same thing as the window that reports `is_focused`: while LoFi's layer-shell overlay holds exclusive keyboard focus *no* window reports `is_focused`, yet the action still lands on the column the user was last working in. That is also the window LoFi picks as its target, because switching to a non-empty workspace focuses a window there and so bumps its `focus_timestamp`. The two can only diverge when the active workspace is empty — in which case there is no column and the action is a harmless no-op.

`gather_commands` returns an empty `Vec` when no non-LoFi window is open, matching GNOME, so the rows simply don't appear rather than appearing and doing nothing. Unlike GNOME it does **not** drop the set when geometry can't be read: `work_area` is informational here and `current_frame` is zeroed, because no emitted command reads either. See `app/core/README.md`'s `Command` section for what each field carries on this path and why.

`run_workspace_command` is one call, not GNOME's two: Niri's `MoveWindowToWorkspace` takes a `focus` flag, so "move it and take me with it" is atomic. The workspace-move rows reuse `lofi_core::build_workspace_commands` for the boundary logic, the id scheme, and the relative rows; only the absolute rows' **labels** are rewritten afterwards, so they name the same workspace the switch rows do (core builds them from the positional index, which on a multi-output session is not the number Niri shows the user).

### Summon window under Niri

Niri gets one `Summon window: <title>` row per other open window (MRU order, icon from the summoned window's app). Activating it brings that window into the column directly right of the target window and focuses it; the columns that were right of the target shift one further right. `A* B` on workspace 1 and `C` on workspace 2 become `A C* B`. GNOME's `gather_summon_commands` returns nothing: GNOME floats every window, so there are no columns.

Right of the target, rather than in its place, so the window you were working in stays put and the summoned one arrives beside it — the same place Niri itself opens a new window.

The decision is a pure function, `summon_plan(target, summoned)`, over each window's workspace id and 1-based column. Those come from the `layout.pos_in_scrolling_layout` field of Niri's `Windows` response and are cached on the backend by `gather_windows`, keyed by window id, rather than added to `lofi_core::Window` — that type is shared with GNOME and macOS, which have no columns.

| Summoned window is… | Actions |
| --- | --- |
| on another workspace | `MoveWindowToWorkspace { focus: false }` to the target's workspace, `FocusWindow`, `MoveColumnToIndex(target column + 1)` |
| on the target's workspace, right of it | `FocusWindow`, `MoveColumnToIndex(target column + 1)` |
| on the target's workspace, left of it | `FocusWindow`, `MoveColumnToIndex(target column)` |

Why these exact steps:

- **The column index is taken from the gather-time snapshot.** Niri inserts a window moved in from another workspace as a new column right after the active one, and the target is the active column, so it usually lands in place already and the `MoveColumnToIndex` is a no-op — but naming the index means the result does not hinge on where Niri chose to insert.
- **From the left, the index is the target's own.** Taking the window out of its column first shifts every column right of it one to the left, the target included, so the target's old index is now the slot right of it. Using target + 1 would land the window one column too far.
- **`MoveColumnToIndex` is focused-only**, the second exception to the by-id rule. That is safe because `FocusWindow` on the summoned window is dispatched first, so "focused" is the window LoFi just named. It also leaves the user on the summoned window, which is the point of summoning it.
- **`focus: false` on the move, with a separate focus step.** Niri only lets focus follow a move when the moved window was already focused, which — with LoFi's overlay up — it is not.

No row is offered where summoning would at most focus the window, which its own Window row already does: for the target itself; and on the target's workspace, when either window is floating (no column order to rearrange), when the two share a column, or when the window is already directly right of the target. Nor is one offered when the target has no workspace. A floating window from another workspace is still offered — it is moved over and focused, and stays floating, so there is no column to place.

One accepted limitation: `MoveColumnToIndex` moves a whole column, so summoning a window that shares its column on the *same* workspace brings its column-mates with it. Niri has no by-id way to move a single tile. (From another workspace this does not arise — `MoveWindowToWorkspace` moves just the one window, which arrives as its own column.)

### Power under Niri

Niri is a compositor, not a desktop environment: no session manager, no screensaver service. What is left is systemd-logind — already the GNOME backend's Suspend path, and shared with it in `backend/logind.rs` — plus Niri's own IPC.

- **Suspend / Restart / Shutdown** → logind's `Suspend` / `Reboot` / `PowerOff`, each with `interactive = false`. That bool is logind's polkit-prompt flag; `false` is right because the launcher window has already closed by the time the call lands, so there is nothing to parent a prompt to — and on a normal desktop the active local user is permitted all three without one. Unlike GNOME there is no session manager to interpose a confirmation dialog, so these fire immediately.
- **Log Out** → Niri's `Quit { skip_confirmation: false }`, leaving Niri's own "press Enter to confirm" prompt in place. Same reasoning as GNOME's `Logout(0)`.
- **Lock** → spawns a locker, because Niri implements `ext-session-lock-v1` but ships none. The chain is:
  1. `$LOFI_LOCK_COMMAND`, run through `sh -c` so it can carry arguments without LoFi inventing a quoting convention.
  2. `lock-command` from [the config file](#configuration), run the same way.
  3. The first of `swaylock`, `hyprlock`, `waylock`, `gtklock` found on `$PATH`. Candidate order dominates directory order — we want the user's preferred locker, not whichever sits earliest on `$PATH`.
  4. logind's `Session.Lock`.

  An all-whitespace value counts as unset on both of the first two steps. For the variable that is the shell's way of saying "not set"; for the file it is the least surprising reading of `lock-command = ""`, and the alternative is running `sh -c ""`, which exits 0 and locks nothing while hiding the `$PATH` search that would have worked.

  The variable outranks the file so a one-off `LOFI_LOCK_COMMAND=… lofi` still overrides a configured locker. That cuts one way worth knowing on a first switch from the variable to the file: a `LOFI_LOCK_COMMAND` already exported into the running session shadows the new file value until the next login, because LoFi is spawned by the compositor and inherits its environment. The file does not have that problem — LoFi reads it itself, at activation, so every later change lands on the next invocation. That is exactly why `programs.lofi.lockCommand` in the home-manager module now writes the file instead of exporting the variable into both the shell and systemd user environments, as it used to: the old route only reached LoFi through the compositor's environment, so a change took effect at the next *login* rather than the next rebuild.

  Step 4 is **last** deliberately. logind's `Lock` only emits a signal; it locks nothing unless a daemon (`swayidle`, `hypridle`, `xss-lock`) is listening for it. On a session with no such daemon it would succeed and do nothing, which is the worst possible outcome for a Lock command — the user walks away believing the screen is locked. When LoFi falls through to it, it says so on stderr and names `lock-command` as the fix.

  The spawned locker is not waited on: LoFi exits moments later and the locker is reparented to init, which is what we want — blocking would keep a launcher process alive for the whole locked period.

## App-to-recent-window combine step

`main.rs::on_activate` does one piece of cross-cutting work between the gatherers: it stamps each `Application` with the id of its most recently focused window (if any) so the UI and `launch` can act on it. The shape is:

1. Walk `windows` in order, which every backend guarantees is MRU order.
2. Build a `HashMap<String, u64>` keyed by `app_desktop_id`, inserting only on the **first** occurrence of each id — later entries are less recent and must not clobber the earlier one. The let-chain guard with `!mru.contains_key(id)` enforces this.
3. For each `Application`, set `app.recent_window_id = mru.get(&app.desktop_id).copied()` and keep `is_running` in lockstep.

This lives in `main` rather than in either gatherer because the two `Vec`s are otherwise independent — `apps::gather_applications` is desktop-agnostic enough that it shouldn't know about window tracking, and no backend has the application list to annotate. The combine step is the cheapest possible glue: one map allocation and one linear pass per `Vec`.

It is also why `app_desktop_id` has to be *canonical* on both backends: the join is a string equality against `Application::desktop_id`.

## MRU activation history

The launcher persists an MRU (most-recently-used) record of activations and feeds that recency into `lofi_core::rank` as the dominant ranking signal for the displayed list (fuzzy score orders only the never-activated entries; a prefix sub-signal sharpens the order within the MRU set). The store itself — schema, write/read API, locking strategy — and the ranking algorithm both live in `lofi-core` (see `app/core/README.md`); this section covers only what the Linux platform layer does with them.

### Path resolution

`main::mru_state_path` returns `$XDG_STATE_HOME/lofi/mru.sqlite` when `$XDG_STATE_HOME` is set and non-empty, otherwise `$HOME/.local/state/lofi/mru.sqlite`, otherwise `None`. The shape mirrors `apps::application_directories` deliberately — the launcher already does manual XDG resolution there, so a second crate (`xdg`, `directories`) would be the bigger dependency than the duplicated logic. Returning `None` instead of panicking is the same policy as everywhere else in this binary: a missing-`HOME` environment is degraded but not fatal.

### Open and read once per invocation

`main::on_activate` opens the `MruStore` immediately after gathering applications and windows. Both the `open` and the subsequent `read_all` are wrapped in `.map_err(|e| eprintln!(...)).ok()`, so any failure (no resolvable path, permission denied on the parent dir, corrupt SQLite header, disk full) downgrades to logging and leaves the UI with `None` for the store and an empty `Vec<EntryRef>` for the index. The launcher never refuses to come up because the history is broken; the worst case is "first run after a corrupt DB shows no recency order this session".

The read is a snapshot: if another LoFi process bumps the DB concurrently, this process's UI does not see the change until its next session. That's fine — concurrent launches are rare, and re-reading on every keystroke would be solving a problem we don't have.

### Sorting and bumping in `ui::build`

`ui::build` accepts `Option<Rc<MruStore>>` and `Vec<EntryRef>` alongside `entries`. It converts the index into a `HashMap<EntryRef, usize>` keyed on rank (0 = most recent) and stores it on `UiState` as `mru_position`. `populate_list` does not sort the list itself — it hands `(&entries, query, &mru_position)` straight to `lofi_core::rank`, which decides both what's visible (filter) and the order (MRU recency, prefix, then score). The Linux layer's whole job here is to construct the rank map and pass it through.

On both Enter (`SearchEntry::connect_activate`) and click (`ListBox::connect_row_activated`) the helper `bump_mru` runs **synchronously, immediately before `launch::activate`**. The UPSERT is microseconds — the user never notices — and synchronous is simpler than fire-and-forget when the connection lives in a closure that's about to be dropped as the window closes. If the bump fails (disk full, the DB went away between open and click) we `eprintln!` and proceed with the launch; surfacing a "could not record history" error to the user would be obnoxious for something this peripheral.

### Window vs. Application bumping

A `Entry::Window` activation only bumps that Window's row. It does **not** also bump the underlying Application: the two are independent rows in the same table, and the launcher treats "picked the Chrome — github.com window" as evidence that window is recent, not as evidence Chrome-the-app is recent. The opposite (bumping an app while also bumping its most recent window) was rejected for the same reason: coupling would muddle the recency signal the user gives us per row.

### Workspace, PowerCommand, and WorkspaceCommand activations

All three ride the **same** `EntryRef`-keyed MRU path as Applications and Windows, with no special-casing in `ui.rs::bump_mru`. `Entry::reference()` already returns the right variant, the MRU SQLite table is generic over the tagged-enum serialization of `EntryRef` (see `app/core/README.md`), and the rank map keys off `EntryRef` regardless of variant. That is why none of them needed MRU plumbing when they landed, and why a future variant won't either.

Two id schemes are worth calling out:

- `EntryRef::Workspace` keys on the workspace **index**, so picking "Workspace 2" bumps `{"type":"workspace","id":1}`. Under Niri that index is the backend's positional one — deliberately, since Niri's own workspace ids churn (see above).
- `WorkspaceCommand::as_id` splits absolute from relative. The **absolute** moves key on the destination index — "Move to workspace 3" bumps `{"type":"workspace_command","id":"move_to_workspace_2"}` — so each workspace target is remembered as its own row. The **relative** moves use fixed ids (`move_to_previous_workspace` / `move_to_next_workspace`) independent of which workspace the window happened to be on, so "move next" is remembered as an action rather than as one specific destination. The absolute id space and the workspace-switch id space are distinct `EntryRef` variants, so "Move to workspace 3" and switching to "Workspace 3" are independent MRU rows that never collide.

### Persistence note: stale window rows

Window rows from prior compositor sessions are dead weight. Window ids are session-ephemeral on both desktops (see `Window::id` in `app/core/README.md`), so a `EntryRef::Window(12345)` written yesterday will never resolve against today's gather. The launcher tolerates this — `resolve` simply returns `None` for those refs and the UI ignores them — and the rows just accumulate in the table. Periodic cleanup (delete oldest N when the table exceeds 2N rows, or drop unresolved Window refs at startup) is a future pass; we don't yet have enough sense of the steady-state size to commit to a policy.

## Desktop version support

LoFi targets exactly one version of each desktop at a time — whatever is current on the developer's NixOS system. There is no compatibility shim for older or newer releases.

- **GNOME**: the extension's `shell-version` in `metadata.json` is the source of truth.
- **Niri**: the JSON IPC is documented as backwards-compatible (fields and variants are added, not renamed or removed), and the hand-written structs here ignore unknown fields, so the client should survive Niri upgrades. The `Action` request shapes are the fragile half — they must match the compositor's `Action` enum exactly — and the wire-format unit tests in `ipc.rs` are what catch a drift there.
