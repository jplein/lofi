# Power-management Commands — Lock / Suspend / Restart / Shutdown

## Context

LoFi has Application, Window, Workspace, and Command (window-action) entries. The user wants four system-level commands: Lock, Suspend, Restart, Shutdown. These differ from window-action `Entry::Command` because they don't need a target window, work area, or current frame — they always apply, regardless of focus state. A new entry variant (`Entry::PowerCommand`) keeps the data shapes clean.

Restart and Shutdown route through `org.gnome.SessionManager` so the standard GNOME 60-second confirmation dialog fires (matches system-menu behavior, protects against accidental triggers). Lock uses `org.gnome.ScreenSaver`. Suspend uses logind (no SessionManager equivalent; polkit typically permits suspend without a prompt for active users).

## Confirmed decisions

1. **New variant `Entry::PowerCommand(PowerCommand)`** — separate from `Entry::Command` because no target state is needed. Mirrors the wrapping-struct pattern of Application/Window/Workspace/Command for future field expansion.
2. **Four kinds** in `PowerCommandKind`: `LockSession`, `Suspend`, `Restart`, `Shutdown`.
3. **Always available** — no gather guard. The four commands always appear in the launcher.
4. **MRU persists via `EntryRef::PowerCommand(String)`** — same snake_case-keyed pattern as the other variants. JSON `{"type":"power_command","id":"suspend"}`.
5. **D-Bus dispatch** (no new extension changes — these all hit existing system/GNOME services):

   | Action  | Bus     | Service                       | Path                       | Method                |
   |---------|---------|-------------------------------|----------------------------|-----------------------|
   | Lock    | session | `org.gnome.ScreenSaver`       | `/org/gnome/ScreenSaver`   | `Lock()`              |
   | Suspend | system  | `org.freedesktop.login1`      | `/org/freedesktop/login1`  | `Suspend(false)`      |
   | Restart | session | `org.gnome.SessionManager`    | `/org/gnome/SessionManager`| `Reboot()`            |
   | Shutdown| session | `org.gnome.SessionManager`    | `/org/gnome/SessionManager`| `Shutdown()`          |

   Use the lower-level `zbus::blocking::Proxy::call_method` rather than generating four `#[zbus::proxy]` traits — each call is one line and a generated trait would be more noise than signal.
6. **Display names + icons**:

   | Kind          | display_name | icon_name                       |
   |---------------|--------------|---------------------------------|
   | LockSession   | Lock         | `system-lock-screen-symbolic`   |
   | Suspend       | Suspend      | `weather-clear-night-symbolic`  |
   | Restart       | Restart      | `system-reboot-symbolic`        |
   | Shutdown      | Shutdown     | `system-shutdown-symbolic`      |
7. **snake_case ids**: `lock_session`, `suspend`, `restart`, `shutdown`.

## File-by-file

### `lofi-core`

#### `app/core/src/lib.rs`

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerCommandKind {
    LockSession,
    Suspend,
    Restart,
    Shutdown,
}

impl PowerCommandKind {
    pub fn as_id(&self) -> &'static str { /* "lock_session" / "suspend" / "restart" / "shutdown" */ }
    pub fn display_name(&self) -> &'static str;
    pub fn icon_name(&self) -> &'static str;
    pub fn from_id(id: &str) -> Option<PowerCommandKind>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerCommand {
    pub kind: PowerCommandKind,
}
```

Variants:
- `EntryKind::PowerCommand`
- `Entry::PowerCommand(PowerCommand)`
- `EntryRef::PowerCommand(String)`

Accessor arms in `Entry::{name, icon, kind, reference}` (exhaustive, no `_`):
- `name` → `c.kind.display_name()`
- `icon` → `Some(c.kind.icon_name())`
- `kind` → `EntryKind::PowerCommand`
- `reference` → `EntryRef::PowerCommand(c.kind.as_id().to_string())`

#### `app/core/src/matcher.rs`

Haystack gains: `Entry::PowerCommand(c) => c.kind.display_name().to_string()`. Match must remain exhaustive.

### `lofi-gnome`

#### `app/gnome/src/power.rs` (new)

```rust
use lofi_core::{PowerCommand, PowerCommandKind};
use zbus::blocking::{Connection, Proxy};

const ALL_KINDS: &[PowerCommandKind] = &[
    PowerCommandKind::LockSession,
    PowerCommandKind::Suspend,
    PowerCommandKind::Restart,
    PowerCommandKind::Shutdown,
];

/// Static set of power commands. Always returned in full — they don't depend
/// on the focused window or any runtime state.
pub fn gather_power_commands() -> Vec<PowerCommand> {
    ALL_KINDS.iter().map(|&kind| PowerCommand { kind }).collect()
}

/// Dispatch a power command via D-Bus. Logs and returns on failure; never
/// panics, matches the rest of the codebase's `eprintln!`-and-degrade policy.
pub fn activate(kind: PowerCommandKind) {
    let result = match kind {
        PowerCommandKind::LockSession => lock_session(),
        PowerCommandKind::Suspend => suspend(),
        PowerCommandKind::Restart => restart(),
        PowerCommandKind::Shutdown => shutdown(),
    };
    if let Err(e) = result {
        eprintln!("power: {kind:?} failed: {e}");
    }
}

fn lock_session() -> zbus::Result<()> {
    let conn = Connection::session()?;
    Proxy::new(&conn, "org.gnome.ScreenSaver", "/org/gnome/ScreenSaver", "org.gnome.ScreenSaver")?
        .call_method("Lock", &())?;
    Ok(())
}

fn suspend() -> zbus::Result<()> {
    // Suspend lives on the SYSTEM bus, not session. `interactive=false` skips
    // the polkit prompt — suspend is almost always allowed for active users.
    let conn = Connection::system()?;
    Proxy::new(&conn, "org.freedesktop.login1", "/org/freedesktop/login1",
               "org.freedesktop.login1.Manager")?
        .call_method("Suspend", &(false,))?;
    Ok(())
}

fn restart() -> zbus::Result<()> {
    let conn = Connection::session()?;
    Proxy::new(&conn, "org.gnome.SessionManager", "/org/gnome/SessionManager",
               "org.gnome.SessionManager")?
        .call_method("Reboot", &())?;
    Ok(())
}

fn shutdown() -> zbus::Result<()> {
    let conn = Connection::session()?;
    Proxy::new(&conn, "org.gnome.SessionManager", "/org/gnome/SessionManager",
               "org.gnome.SessionManager")?
        .call_method("Shutdown", &())?;
    Ok(())
}
```

Lock/Restart/Shutdown use the session bus; Suspend uses the system bus. `call_method`'s body argument is `&()` for no-args, `&(false,)` for the single bool. Returns are unit; we drop the response.

#### `app/gnome/src/lib.rs`

Add `pub mod power;`.

#### `app/gnome/src/launch.rs`

Add the new arm (exhaustive):
```rust
Entry::PowerCommand(c) => power::activate(c.kind),
```

#### `app/gnome/src/main.rs`

After the existing gathers:
```rust
let power_commands = power::gather_power_commands();
```

Update the `Vec::with_capacity` to include `power_commands.len()`. Extend entries:
```rust
entries.extend(power_commands.into_iter().map(Entry::PowerCommand));
```

Add `power` to the `use lofi_gnome::{...}` import line.

#### `app/gnome/src/ui.rs`

`kind_to_str` gains `EntryKind::PowerCommand => "Power"`.

## Tests

### `app/core/src/lib.rs` `mod tests`

Add `make_power_command(kind) -> PowerCommand` helper and `ALL_POWER_COMMAND_KINDS: &[PowerCommandKind]` constant (mirror of `ALL_COMMAND_KINDS`).

1. `entry_power_command_reference_round_trips` — for every kind: build `Entry::PowerCommand(make_power_command(k))`, assert `entry.reference() == EntryRef::PowerCommand(k.as_id().into())`, round-trip via `resolve`.
2. `resolve_finds_power_command_by_reference` — mixed `Application`/`Window`/`Workspace`/`Command`/`PowerCommand` entries; resolve a specific PowerCommand by ref. Cross-variant guards in both directions: `EntryRef::Command("suspend")` doesn't resolve to `PowerCommandKind::Suspend`, and `EntryRef::PowerCommand("center")` doesn't resolve to `CommandKind::Center`.
3. `entry_ref_power_command_serializes_to_tagged_json` — exact `r#"{"type":"power_command","id":"suspend"}"#`; round-trip.
4. `entry_power_command_methods_return_command_data` — name/icon/kind for Lock, Suspend, Shutdown per the table.
5. `power_command_kind_id_round_trips_through_from_id` — all 4 variants + negative case (`from_id("not-a-power-command") == None`).

### `app/core/src/matcher.rs` `mod tests`

Add `power(kind) -> Entry` helper.

1. `matcher_finds_power_command_by_name` — entries include the four PowerCommands plus an Application "Lockheed Martin" sanity-check that shouldn't shadow Lock. Query `"lock"` includes Lock (and possibly the Application — only assert about the PowerCommand subset). Query `"suspend"` includes Suspend only.

### `app/gnome/`

No new tests — live D-Bus.

## Implementation order

1. `app/core/src/lib.rs` — types + variants + accessors + helper + 5 tests.
2. `app/core/src/matcher.rs` — haystack arm + helper + 1 test.
3. `cargo test -p lofi-core` clean.
4. `app/gnome/src/power.rs` — new module.
5. `app/gnome/src/lib.rs` — `pub mod power;`.
6. `app/gnome/src/launch.rs` — `Entry::PowerCommand` arm.
7. `app/gnome/src/main.rs` — gather + extend.
8. `app/gnome/src/ui.rs` — `kind_to_str` arm.
9. `cargo build/test/clippy/fmt --workspace` clean.
10. `nix build` clean (verify `git add` for new files).
11. READMEs (`app/core/README.md` — PowerCommand subsection; `app/gnome/README.md` — power module + launch arm).

## Verification

- `cargo test -p lofi-core` — 5 new lib + 1 new matcher tests + existing all pass.
- `cargo test --workspace` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `nix build` — clean.
- **Manual** (Wayland; no extension reinstall needed — no extension changes):
  - Open LoFi → type `lock` → select Lock → screen locks immediately.
  - Open LoFi → type `suspend` → select Suspend → system suspends.
  - Open LoFi → type `restart` → select Restart → GNOME's confirmation dialog appears ("Restart? Cancel").
  - Open LoFi → type `shutdown` → select Shutdown → GNOME's "Power Off" confirmation dialog appears.
  - Each command bumps in MRU after first use — reopen LoFi to see it at the top.

## Out of scope

- Customizable confirmation behavior per command.
- Logout (separate from session lock — could land later as `EntryRef::PowerCommand("logout")`).
- Hibernate (logind supports it but it's less commonly enabled and the icon is fuzzier).
- Reboot-to-firmware or reboot-to-boot-loader options.
- Cleanup of stale `EntryRef::PowerCommand(...)` rows — the id set is closed; stale rows never resolve so they're harmless dead weight.
- Custom polkit policy installation if Suspend gets blocked.
