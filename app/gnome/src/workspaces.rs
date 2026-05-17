//! D-Bus client for the GNOME Shell extension's workspace methods on the
//! `WindowManager` interface.
//!
//! Mirrors `windows.rs` exactly — same session-bus service, same blocking
//! proxy pattern, same `eprintln!`-and-degrade error policy. Kept in its own
//! module (with its own proxy trait) so the windows and workspaces concerns
//! stay independent; the wire interface happens to be shared, but nothing in
//! either module depends on the other.
//!
//! No `unwrap`/`expect` here: any D-Bus error is logged via `eprintln!` and
//! the call returns an empty Vec (`gather_workspaces`) or unit
//! (`activate_workspace`). The launcher then shows the remaining (Application/
//! Window) entries — degraded but functional.

use lofi_core::Workspace;
use zbus::blocking::Connection;
use zbus::zvariant::{DeserializeDict, Type};

/// Blocking D-Bus proxy for the extension's workspace methods. Same interface
/// and service as the windows proxy in `windows.rs` — the extension exposes
/// both surfaces from a single object — but declared independently here so
/// the modules don't depend on each other.
#[zbus::proxy(
    interface = "dev.jplein.LoFi.Shell.WindowManager",
    default_service = "dev.jplein.LoFi.Shell",
    default_path = "/dev/jplein/LoFi/Shell",
    gen_blocking = true,
    gen_async = false
)]
trait WorkspaceManager {
    /// Return every workspace the extension sees, serialized as an `a{sv}`
    /// dict per workspace. The extension also emits `active` and `n_windows`
    /// in the dict; zvariant ignores dict keys not declared on `DbusWorkspace`
    /// so we silently drop them on decode.
    fn list_workspaces(&self) -> zbus::Result<Vec<DbusWorkspace>>;

    /// Switch GNOME to the workspace with the given 0-based `index`.
    fn activate_workspace(&self, index: i32) -> zbus::Result<()>;
}

/// Wire shape of a single workspace over D-Bus. Mirrors the dict the
/// extension builds in `extension/gnome/src/workspaces.ts`. The extension
/// also emits `active` and `n_windows`; those are skipped here because the
/// Rust `Workspace` doesn't need them today. zvariant's dict decoder ignores
/// dict keys not declared on the target struct, so adding fields here later
/// is a one-line change.
#[derive(Debug, Type, DeserializeDict)]
#[zvariant(signature = "a{sv}")]
struct DbusWorkspace {
    index: i32,
    name: String,
}

/// Open a fresh session-bus connection. Mirrors `windows::connect`: kept
/// private and per-call because the launcher is short-lived enough that
/// connection reuse buys nothing.
fn connect() -> zbus::Result<Connection> {
    Connection::session()
}

/// Convert a wire `DbusWorkspace` into the public `lofi_core::Workspace`. No
/// empty-string-to-`None` coercion needed: both fields are non-optional in
/// `Workspace`.
fn map_dbus_workspace(w: DbusWorkspace) -> Workspace {
    Workspace {
        index: w.index,
        name: w.name,
    }
}

/// Ask the extension for the current workspace list. Any D-Bus failure
/// (connection refused, no name owner, malformed reply) yields an empty Vec
/// after an `eprintln!`. The launcher then shows only Application/Window
/// entries — degraded but functional.
pub fn gather_workspaces() -> Vec<Workspace> {
    let connection = match connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lofi: connect to session bus failed: {e}");
            return Vec::new();
        }
    };

    let proxy = match WorkspaceManagerProxy::new(&connection) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lofi: create WorkspaceManager proxy failed: {e}");
            return Vec::new();
        }
    };

    let raw = match proxy.list_workspaces() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lofi: list_workspaces failed: {e}");
            return Vec::new();
        }
    };

    raw.into_iter().map(map_dbus_workspace).collect()
}

/// Ask the extension to switch to the workspace with `index`. Errors are
/// logged and swallowed — there's no caller-side recovery from "the workspace
/// vanished between gather and click".
pub fn activate_workspace(index: i32) {
    let connection = match connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lofi: connect to session bus failed: {e}");
            return;
        }
    };

    let proxy = match WorkspaceManagerProxy::new(&connection) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lofi: create WorkspaceManager proxy failed: {e}");
            return;
        }
    };

    if let Err(e) = proxy.activate_workspace(index) {
        eprintln!("lofi: activate workspace {index} failed: {e}");
    }
}
