# Icon identifier on Application

## Summary

Add `icon: Option<String>` to `lofi_core::Application` carrying the freedesktop icon identifier (themed name or absolute file path) parsed from the `.desktop` file's `Icon=` line. On the GNOME side, populate from `DesktopAppInfo::icon()` via `gio::prelude::IconExt::to_string()`. No new dependencies, no `Cargo.toml` changes, no `main.rs` changes. The integration test gains an `icon` parameter on its fixture helper and asserts a sorted-by-`desktop_id` `Vec<Option<String>>`.

Identifier, **not** bytes: GTK's `IconTheme` / `Image::from_icon_name` resolves themed names at render time with the correct size, theme, scale, and dark-mode behavior. Resolving here would force eager I/O, lock in a size, and go stale on theme changes.

## File-by-file changes

### 1. `/home/jplein/Git/jplein/lofi/app/core/src/lib.rs`

Replace the current struct with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub name: String,
    pub desktop_id: String,
    pub icon: Option<String>,
}
```

- `Option<String>`. `None` means: missing `Icon=` line, unsupported `Icon` subclass, or empty/whitespace string.
- Plain `String` (not an enum): `gio::IconExt::to_string` is the canonical freedesktop serializer; downstream callers can re-parse with `gio::Icon::for_string` if they need the typed form. Keeps `lofi-core` dep-free and platform-agnostic.

### 2. `/home/jplein/Git/jplein/lofi/app/gnome/src/apps.rs`

#### Imports
No new `use` lines. `use gtk::gio::prelude::*;` at line 6 already brings in `IconExt`.

#### Extraction expression
Inside `gather_applications`, before the `out.push(Application { ... })` line, add:

```rust
let icon: Option<String> = info
    .icon()
    .and_then(|i| IconExt::to_string(&i))
    .map(|gs| gs.to_string())
    .filter(|s| !s.trim().is_empty());
```

- `info.icon()` returns `Option<gio::Icon>`.
- `IconExt::to_string(&i)` is the freedesktop serializer (`g_icon_to_string`); for a `ThemedIcon` it returns the name, for a `FileIcon` it returns the absolute path, for other subclasses `None`. The explicit form (rather than `i.to_string()`) is preferred for clarity — but if clippy flags it, switch to `i.to_string()`; behavior is identical.
- `GString` → `String` via `.to_string()` (or `String::from(...)`).
- Empty-string coercion guards against malformed `Icon=` lines.

#### Fallback if `IconExt::to_string` returns `GString` directly (non-Option) in gtk4-rs 0.11
The coder picks whichever shape compiles. Alternative:

```rust
let icon: Option<String> = info
    .icon()
    .map(|i| IconExt::to_string(&i).to_string())
    .filter(|s| !s.trim().is_empty());
```

Both produce identical observable behavior given the empty-string filter.

#### Struct literal
Change `out.push(Application { name, desktop_id });` to:

```rust
out.push(Application { name, desktop_id, icon });
```

No other changes to `apps.rs`.

### 3. `/home/jplein/Git/jplein/lofi/app/gnome/tests/apps.rs`

#### Helper signature

```rust
fn write_desktop(dir: &Path, filename: &str, name: &str, exec: &str, icon: &str)
```

Body emits `Icon={icon}` in addition to the existing lines:

```text
[Desktop Entry]
Type=Application
Name={name}
Exec={exec}
Icon={icon}
```

#### Fixture calls
Pass distinct themed-name-style icon identifiers (no slashes, no extension) so we know `DesktopAppInfo::icon()` returns a `ThemedIcon` that round-trips verbatim:

- `alpha.desktop` → `"test-icon-alpha"`
- `beta.desktop` → `"test-icon-beta"`
- `gamma.desktop` → `"test-icon-gamma"`

These don't exist in any real theme, which is irrelevant — we assert the identifier, never resolve it.

#### New assertion
After the existing `stems` assertion:

```rust
let icons: Vec<Option<String>> = apps.iter().map(|a| a.icon.clone()).collect();
assert_eq!(
    icons,
    vec![
        Some("test-icon-alpha".to_string()),
        Some("test-icon-beta".to_string()),
        Some("test-icon-gamma".to_string()),
    ],
    "icons sorted by desktop_id should match the fixtures; got {icons:?}"
);
```

(`apps` is already sorted by `desktop_id` earlier in the test.)

#### Test scope decisions
- No `None`-icon fixture this iteration. Would require an `Option<&str>` parameter or a second helper variant.
- No `FileIcon` (absolute-path) fixture. Identical code path on the Rust side; redundant.
- No whitespace-coercion assertion. Internal defensive measure with no realistic upstream input under tempdir.

### 4. `/home/jplein/Git/jplein/lofi/app/core/README.md`

Update the **Current contents** section. Briefly:
- Still notes only `Application` is defined.
- Lists its three fields: `name` (display), `desktop_id` (stable identifier), `icon` (`Option<String>` — an icon **identifier**, not bytes; typically a freedesktop themed-icon name like `"firefox"` or, less commonly, an absolute file path).
- One sentence on the identifier-not-bytes choice: rendering happens in the UI layer where icon theme, scale, and target size are known; resolving here would force eager I/O and lock in stale answers.
- Keeps the forward-looking sentence about `Window`, `Workspace`, `Command`.

### 5. `/home/jplein/Git/jplein/lofi/app/gnome/README.md`

In the `apps` module bullet, update the `gather_applications(dirs)` sub-bullet to note that each returned `Application` includes `icon: Option<String>` populated from `DesktopAppInfo::icon()` via `gio::IconExt::to_string`. One short sentence emphasizing identifier-not-bytes — rendering is deferred to the GTK image widget at draw time.

### Unchanged
- `app/Cargo.toml`, `app/core/Cargo.toml`, `app/gnome/Cargo.toml`
- `app/gnome/src/lib.rs` (type re-exports are name-based)
- `app/gnome/src/main.rs`
- `flake.nix`, `rust-toolchain.toml`

## Implementation order

1. Edit `app/core/src/lib.rs` (add field). Workspace temporarily fails to build.
2. Edit `app/gnome/src/apps.rs` (extract icon + update struct literal). Workspace builds again.
3. Edit `app/gnome/tests/apps.rs` (helper signature, fixture calls, icons assertion).
4. Edit `app/core/README.md`.
5. Edit `app/gnome/README.md`.

## Verification (from `/home/jplein/Git/jplein/lofi/app/`)

1. `cargo build --workspace`
2. `cargo test --workspace` — icons assertion must pass.
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo fmt --all -- --check`
5. `cargo build -p lofi-gnome --bin lofi` — hello-world still builds.

## Out of scope

- UI rendering of icons (`gtk::Image` wiring) — separate feature.
- Eager byte-resolution, theme lookup, size selection, caching, fallback chains — render-time concerns.
- macOS gatherer / `app/macos/`.
- `None`-icon test fixture, `FileIcon` test fixture.
- Renaming or re-deriving traits on `Application`.
- `Cargo.toml`, `flake.nix`, `rust-toolchain.toml`, `main.rs` changes.
