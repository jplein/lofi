# Application listing — first pass

## Problem statement

LoFi is a small launcher for GNOME (Linux) and macOS, written in Rust. The Linux side currently has only a hello-world GTK window. This change adds the first functional capability: enumerating installed applications by parsing `.desktop` files on disk into a Rust data structure.

It also introduces the shared `app/core` crate, which houses the cross-platform `Application` data type.

## Confirmed design constraints

1. **`app/core` is platform-agnostic.** It defines the `Application` struct and nothing more. It has no `gtk`, `gio`, or any other platform-specific dependency.
2. **The gatherer (`gather_applications`) lives in `app/gnome`** because it uses `gtk::gio::DesktopAppInfo`, which is Linux-only.
3. **Pure-function shape**: `gather_applications(dirs: &[PathBuf]) -> Vec<Application>` takes directories as input. A separate `application_directories() -> Vec<PathBuf>` reads `XDG_DATA_HOME` / `XDG_DATA_DIRS` for runtime use.
4. **Integration test writes `.desktop` files into subdirs of a `tempfile::tempdir()`** and asserts all are returned. Test must not depend on `XDG_CURRENT_DESKTOP`.
5. **`app/gnome` becomes a lib + bin crate** so the integration test can import the gatherer. `main.rs` is unchanged.
6. **`Application` minimum fields**: `name: String`, `desktop_id: String`. Derive `Debug, Clone, PartialEq, Eq`.

## File-by-file changes

### Create: `/home/jplein/Git/jplein/lofi/app/core/Cargo.toml`

```toml
[package]
name = "lofi-core"
version = "0.1.0"
edition = "2024"

[lib]
name = "lofi_core"
path = "src/lib.rs"
```

**No dependencies.** This crate must remain platform-agnostic.

### Create: `/home/jplein/Git/jplein/lofi/app/core/src/lib.rs`

A single public struct:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub name: String,
    pub desktop_id: String,
}
```

Fields are `pub` — no getters for this two-field POD.

### Modify: `/home/jplein/Git/jplein/lofi/app/Cargo.toml`

Add `"core"` to `members`. Final:

```toml
[workspace]
resolver = "3"
members = ["core", "gnome"]
```

### Modify: `/home/jplein/Git/jplein/lofi/app/gnome/Cargo.toml`

Add `[lib]` target, the `lofi-core` dep, and the `tempfile` dev-dep. Final:

```toml
[package]
name = "lofi-gnome"
version = "0.1.0"
edition = "2024"

[lib]
name = "lofi_gnome"
path = "src/lib.rs"

[[bin]]
name = "lofi"
path = "src/main.rs"

[dependencies]
gtk = { version = "0.11", package = "gtk4" }
adw = { version = "0.9", package = "libadwaita" }
gio-unix = "0.22"
lofi-core = { path = "../core" }

[dev-dependencies]
tempfile = "3"
```

(`gio-unix` is required because `DesktopAppInfo` is gated to Unix and not re-exported from the cross-platform `gtk::gio`. The `AppInfoExt` trait methods still come from `gtk::gio::prelude::*`.)

### Create: `/home/jplein/Git/jplein/lofi/app/gnome/src/lib.rs`

```rust
pub mod apps;

pub use lofi_core::Application;
pub use apps::{application_directories, gather_applications};
```

This file exists so integration tests under `app/gnome/tests/` have a library crate to attach to (Cargo integration tests require a `[lib]` target).

### Create: `/home/jplein/Git/jplein/lofi/app/gnome/src/apps.rs`

Two public functions; uses `gtk::gio::DesktopAppInfo` for parsing.

#### Imports
- `std::env`
- `std::fs`
- `std::path::PathBuf`
- `gio_unix::DesktopAppInfo`
- `gtk::gio::prelude::*` (brings `AppInfoExt`)
- `lofi_core::Application`

#### `application_directories() -> Vec<PathBuf>`

Order:
1. Compute `data_home`:
   - If `XDG_DATA_HOME` is set and non-empty, use it.
   - Else use `$HOME/.local/share`.
   - If `HOME` is unset, omit `data_home`.
2. Compute `data_dirs`:
   - If `XDG_DATA_DIRS` is set and non-empty, split on `:`.
   - Else default to `["/usr/local/share", "/usr/share"]`.
3. Push `data_home` (if present), then each entry of `data_dirs` in order.
4. Skip empty entries (so `a::b` produces two entries, not three).
5. Append `applications` to each surviving entry via `PathBuf::push("applications")`.
6. Do **not** verify existence here (that's `gather_applications`' responsibility).
7. Do **not** deduplicate.

#### `gather_applications(dirs: &[PathBuf]) -> Vec<Application>`

For each `dir` in `dirs`:
- `fs::read_dir(dir)` — on `Err` (missing dir, permission denied, not a directory), `continue` silently. No panic, no log.
- For each entry:
  - Skip if `entry.file_type()` errors or is not a regular file (subdirs are not recursed into).
  - Skip if filename does not end in `.desktop`.
  - `let info = gio_unix::DesktopAppInfo::from_filename(&path);` — skip if `None`.
  - Skip if `info.should_show()` is false. This handles `NoDisplay`, `Hidden`, `OnlyShowIn`, `NotShowIn`, `TryExec` per the freedesktop spec.
  - `name = info.name().to_string()` (`info.name()` returns `glib::GString`).
  - `desktop_id`:
    - Prefer `info.id()` (returns `Option<glib::GString>`); convert to `String` if present.
    - Else fall back to `path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)`.
    - If both fail, skip the file.
  - Push `Application { name, desktop_id }`.

Returns the vector in filesystem-iteration order within each dir, dirs in input order. No sorting, no dedup.

#### Error / panic policy

`gather_applications` never returns `Result` and never panics on file-system errors. Library code should avoid `unwrap()` / `expect()`.

#### Gotcha

`gtk::gio::DesktopAppInfo::from_filename` does not require `gtk::init()` or a main loop. The integration test calls it with no GTK setup.

### Create: `/home/jplein/Git/jplein/lofi/app/gnome/tests/apps.rs`

Single integration test file.

#### Imports
- `lofi_gnome::{Application, gather_applications}`
- `std::fs`
- `std::path::PathBuf`
- `tempfile::tempdir`

#### Helper (file-scope)
`fn write_desktop(dir: &std::path::Path, filename: &str, name: &str, exec: &str)` — `fs::create_dir_all(dir)`, then writes:

```
[Desktop Entry]
Type=Application
Name=<name>
Exec=<exec>
```

(trailing newline). No `NoDisplay`, `Hidden`, `OnlyShowIn`, or `TryExec` keys, so `should_show()` returns true regardless of `XDG_CURRENT_DESKTOP`.

#### Test: `#[test] fn gather_applications_lists_all_desktop_files_in_supplied_dirs()`

1. `let temp = tempdir().unwrap();`
2. Build paths `temp/data_home/applications` and `temp/data_dirs/usr_share/applications`.
3. Write three `.desktop` files:
   - `<data_home>/alpha.desktop` — Name=Alpha, Exec=true
   - `<data_home>/beta.desktop` — Name=Beta, Exec=true
   - `<data_dirs/usr_share>/gamma.desktop` — Name=Gamma, Exec=true

   (`Exec=true` — bare command name — resolves via `PATH`. We can't use `/bin/true` because on NixOS that absolute path does not exist, and gio rejects desktop entries whose absolute `Exec` path is missing.)
4. Write a non-`.desktop` file `<data_home>/readme.txt` (confirms non-`.desktop` files are ignored).
5. Create an empty subdir `<data_home>/subdir/` (confirms walk is non-recursive).
6. Build `dirs` with both `applications` paths plus a non-existent path `<temp>/nonexistent/applications` (confirms missing dirs are silently skipped).
7. Call `gather_applications(&dirs)`.
8. Collect `apps`. Sort by `desktop_id`.
9. Assert:
   - `apps.len() == 3`
   - Names (sorted) equal `["Alpha", "Beta", "Gamma"]`
   - The set of `desktop_id` values, after stripping any trailing `.desktop` and any leading directory components, equals `{"alpha", "beta", "gamma"}`. (`gio::DesktopAppInfo::id()` may return `None` for tempdir paths; our fallback gives bare stems.)

#### Test hygiene
- Must not mutate process env (`XDG_DATA_HOME`, etc.). `gather_applications` doesn't read env at all.
- Must not assume `XDG_CURRENT_DESKTOP` is set.

### Leave unchanged
- `/home/jplein/Git/jplein/lofi/app/gnome/src/main.rs`
- `/home/jplein/Git/jplein/lofi/rust-toolchain.toml`
- `/home/jplein/Git/jplein/lofi/flake.nix`

## Implementation order (for the coder)

1. Create `app/core/Cargo.toml`.
2. Create `app/core/src/lib.rs`.
3. Edit `app/Cargo.toml` — add `"core"` to members.
4. Create `app/gnome/src/apps.rs`.
5. Create `app/gnome/src/lib.rs`.
6. Edit `app/gnome/Cargo.toml` — add `[lib]`, `lofi-core` dep, `tempfile` dev-dep.
7. Create `app/gnome/tests/apps.rs`.
8. `cargo test --workspace` from `app/`.

## Verification (from `app/`)

1. `cargo build --workspace` — clean build.
2. `cargo test --workspace` — the new integration test passes.
3. `cargo fmt --all -- --check` — passes.
4. `cargo clippy --workspace --all-targets -- -D warnings` — passes.
5. `cargo run -p lofi-gnome --bin lofi` — hello-world UI still opens.

## Out of scope (do not grow into these)

- Wiring the listing into the UI (`main.rs` untouched)
- Sorting / deduplication / stable ordering of returned `Vec<Application>`
- `exec`, `icon`, `comment`, `categories` fields on `Application`
- Recursive directory walking
- Fuzzy matching / search indexing
- Application launch
- Caching, inotify watching
- macOS gatherer (`app/macos` doesn't exist yet)
- Unit tests for `application_directories()` (would require mutating process env — parallel-unsafe)
- Any change to `flake.nix` or `rust-toolchain.toml`
- Removing `adw` from `lofi-gnome` deps
