# Workspace entries — list and activate from the launcher

## Context

The GNOME extension already exposes `ListWorkspaces` (returning index, name, active, n_windows) and `ActivateWorkspace(index)` over D-Bus — they're declared in `extension/gnome/dbus-interface.xml`, implemented in `extension/gnome/src/workspaces.ts` and `extension/gnome/src/service.ts`. Nothing on the extension side needs to change.

What's missing is the Rust side: `lofi-core` has no `Workspace`/`Entry::Workspace`/`EntryRef::Workspace`, no `workspaces` module in `lofi-gnome`, no dispatch in `launch.rs`, no gather in `main.rs`. This task wires up what's already on the wire so a user can type "workspace 2" (or any custom workspace name) and switch to it.

MRU integration is free: `EntryRef::Workspace(i32)` plugs into the existing `MruStore` schema (one row per `EntryRef::*`), and `ui.rs` already sorts the visible list by MRU rank with non-MRU at `usize::MAX`. Picking a workspace bumps its row exactly like Application/Window activations.

## Confirmed decisions

1. **No extension changes** — the existing `ListWorkspaces` / `ActivateWorkspace` surface is sufficient.
2. **`Workspace { index: i32, name: String }`** — drop `active` and `n_windows` from the Rust struct for now. The wire dict carries them; zvariant ignores unknown fields on decode. Adding them back later is one field at a time.
3. **`EntryRef::Workspace(i32)`** — keyed by index. Indices shift if workspaces are added/removed, but that's the same trade-off `EntryRef::Window(u64)` already has. A stale row matching a different-but-same-index workspace is acceptable (the user only sees MRU as ordering, not identity).
4. **Workspace icon is a hardcoded constant** (`"view-grid-symbolic"`), returned by `Entry::icon()` for the `Workspace` arm. No `icon` field on the `Workspace` struct. Workspaces don't have variable icons; threading an always-`Some` field through the gatherer would be pure ceremony.
5. **Haystack** = `name` only. With default GNOME the name is `"Workspace 1"`, `"Workspace 2"`, etc. (the extension hardcodes that label). Typing "work", "2", "workspace 2" all match. If a user has an extension that renames workspaces, the custom name flows through.
6. **List all workspaces** unconditionally — including the currently active one, including the single-workspace case. Filtering "useful" ones is presentational; if the user types "workspace 1" they should still find it.

## File-by-file

### `app/core/src/lib.rs`

- New struct:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Workspace {
      pub index: i32,
      pub name: String,
  }
  ```
- `EntryKind` gains `Workspace`.
- `Entry` gains `Workspace(Workspace)`.
- `EntryRef` gains `Workspace(i32)`.
- `Entry::name()` adds the `Workspace(w) => &w.name` arm.
- `Entry::icon()` adds the `Workspace(_) => Some("view-grid-symbolic")` arm (constant).
- `Entry::kind()` adds the `Workspace(_) => EntryKind::Workspace` arm.
- `Entry::reference()` adds the `Workspace(w) => EntryRef::Workspace(w.index)` arm.
- Add tests (see Tests section).

### `app/core/src/matcher.rs`

- `haystack` gains the `Entry::Workspace(w) => w.name.clone()` arm. Name-only.
- Add one new test (see Tests section).

### `app/gnome/src/workspaces.rs` (new)

Mirror `windows.rs` exactly — same zbus blocking-proxy pattern, same empty-string→None coercion philosophy, same `eprintln!`-and-return-empty-Vec error policy.

```rust
use lofi_core::Workspace;
use zbus::blocking::Connection;
use zbus::zvariant::{DeserializeDict, Type};

#[zbus::proxy(
    interface = "dev.jplein.LoFi.Shell.WindowManager",
    default_service = "dev.jplein.LoFi.Shell",
    default_path = "/dev/jplein/LoFi/Shell",
    gen_blocking = true,
    gen_async = false
)]
trait WindowManager {
    fn list_workspaces(&self) -> zbus::Result<Vec<DbusWorkspace>>;
    fn activate_workspace(&self, index: i32) -> zbus::Result<()>;
}

#[derive(Debug, Type, DeserializeDict)]
#[zvariant(signature = "a{sv}")]
struct DbusWorkspace {
    index: i32,
    name: String,
    // `active` and `n_windows` are emitted by the extension but skipped here —
    // zvariant ignores dict keys not declared on the target struct.
}

pub fn gather_workspaces() -> Vec<Workspace> { ... }
pub fn activate_workspace(index: i32) { ... }
```

`gather_workspaces` opens the bus, calls `list_workspaces()`, maps each `DbusWorkspace` to `lofi_core::Workspace`. Any zbus error → `eprintln!` and `Vec::new()`.

`activate_workspace` opens the bus, calls `activate_workspace(index)`. Any zbus error → `eprintln!` and return.

### `app/gnome/src/lib.rs`

Add `pub mod workspaces;`.

### `app/gnome/src/launch.rs`

Add the `Entry::Workspace(w) => workspaces::activate_workspace(w.index)` arm. Match stays exhaustive (no `_` arm).

### `app/gnome/src/main.rs`

After `let windows = windows::gather_windows();`, add:
```rust
let workspaces_vec = workspaces::gather_workspaces();
```
After the apps/windows extension into `entries`:
```rust
entries.extend(workspaces_vec.into_iter().map(Entry::Workspace));
```
Capacity hint: include `workspaces_vec.len()` in `Vec::with_capacity`.

### `app/gnome/src/ui.rs`

`kind_to_str` gains `EntryKind::Workspace => "Workspace"`.

## Tests

### `app/core/src/lib.rs` `mod tests`

Add a `make_workspace(index, name) -> Workspace` helper.

1. `entry_workspace_reference_round_trips` — `Entry::Workspace(make_workspace(2, "Workspace 3"))` → `EntryRef::Workspace(2)`; round-trip via `resolve`. Include `index = 0` as a separate case.
2. `resolve_finds_workspace_by_reference` — mixed `Application`, `Window`, `Workspace` entries; resolve a specific Workspace by ref; assert name and that mismatching variants don't resolve (a `Window(2)` ref does NOT resolve to a `Workspace(2)`).
3. `entry_ref_workspace_serializes_to_tagged_json` — exact-match `r#"{"type":"workspace","id":2}"#`; round-trip.
4. `entry_workspace_methods_return_workspace_data` — `.name`, `.icon` returns `Some("view-grid-symbolic")`, `.kind == EntryKind::Workspace`.

### `app/core/src/matcher.rs` `mod tests`

Add a `workspace(index, name) -> Entry` helper.

5. `matcher_finds_workspace_by_name` — entries include Workspace 1, Workspace 2, an Application "Workspaces App" (sanity check it doesn't shadow); query `"workspace 2"` matches Workspace 2; query `"2"` matches Workspace 2 (not Workspace 1).

### `app/gnome/`

No new tests — live D-Bus, exercised manually like windows/workspaces handling already is.

## Implementation order

1. `app/core/src/lib.rs` — Workspace struct, variants, accessors, helper, 4 new tests. `cargo test -p lofi-core` clean.
2. `app/core/src/matcher.rs` — haystack arm, helper, 1 new test. `cargo test -p lofi-core` clean.
3. `app/gnome/src/workspaces.rs` — new module mirroring `windows.rs`.
4. `app/gnome/src/lib.rs` — `pub mod workspaces;`.
5. `app/gnome/src/launch.rs` — new arm.
6. `app/gnome/src/main.rs` — gather + extend.
7. `app/gnome/src/ui.rs` — `kind_to_str` arm.
8. `cargo build/test/clippy/fmt --workspace` clean.
9. `nix build` clean.
10. READMEs (`app/core/README.md` — Workspace subsection; `app/gnome/README.md` — workspaces module section).

## Verification

- `cargo test -p lofi-core` — 4 new lib tests + 1 new matcher test + existing tests, all pass.
- `cargo test --workspace` — clean (existing apps/MRU integration tests untouched).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `nix build` — clean.
- **Manual** (Wayland + extension reinstalled): open the launcher; verify Workspace entries appear in the list; type "work" or "workspace 2" and confirm filtering; press Enter on Workspace 2 → GNOME switches to workspace 2; reopen launcher → Workspace 2 is now at the top (MRU bump worked).

## Out of scope

- Surfacing `active` or `n_windows` in the UI (drop the dict fields on decode; can revisit if useful).
- Letting LoFi list custom workspace names from GNOME's own naming API (the extension currently hardcodes `"Workspace N"`; changing that is a separate extension-side task).
- Filtering out the active workspace from the list (presentational; user might still want to type "workspace 2" while on it).
- A workspace-specific icon per index (e.g., showing recently-used app icons inside the workspace row).
- "Move active window to workspace N" surface from the launcher (we have the D-Bus method, but it's a different UX — keypress-or-modifier needed to express intent).
- Cleanup of stale `EntryRef::Workspace(N)` rows in MRU when N no longer exists. Same dead-weight tolerance as `EntryRef::Window`.
