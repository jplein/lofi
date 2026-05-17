//! D-Bus client for the GNOME Shell extension's `WindowManager` interface.
//!
//! The extension publishes a session-bus service at
//! `dev.jplein.LoFi.Shell` exporting `dev.jplein.LoFi.Shell.WindowManager` on
//! the path `/dev/jplein/LoFi/Shell`. This module uses zbus's *blocking*
//! API because the GTK main loop is already a synchronous event loop and we
//! gather windows once per launcher invocation; an async runtime would be
//! pure overhead.
//!
//! No `unwrap`/`expect` here: any D-Bus error is logged via `eprintln!` and
//! the call returns an empty Vec (`gather_windows`) or unit (`focus_window`).
//! That matches `apps::gather_applications`' "log and degrade gracefully"
//! behaviour.

use lofi_core::Window;
use zbus::blocking::Connection;
use zbus::zvariant::{DeserializeDict, Type};

// `DeserializeDict`'s derive expands to an `impl Deserialize for DbusWindow`,
// so we can't add `#[derive(serde::Deserialize)]` alongside it — zbus 5
// differs from the plan here. The trait is still implemented; we just don't
// derive it twice.

/// Blocking D-Bus proxy for the extension's `WindowManager` interface. With
/// `gen_async = false`, zbus 5 drops the `Blocking` suffix and names the
/// generated type `WindowManagerProxy` (see `zbus_macros::proxy` source) —
/// not `WindowManagerProxyBlocking` as the plan suggested.
#[zbus::proxy(
    interface = "dev.jplein.LoFi.Shell.WindowManager",
    default_service = "dev.jplein.LoFi.Shell",
    default_path = "/dev/jplein/LoFi/Shell",
    gen_blocking = true,
    gen_async = false
)]
trait WindowManager {
    /// Return every top-level Meta.Window the extension sees, serialized as
    /// an `a{sv}` dict per window.
    fn list_windows(&self) -> zbus::Result<Vec<DbusWindow>>;

    /// Raise the window with `id` and switch to its workspace.
    fn focus_window(&self, id: u64) -> zbus::Result<()>;
}

/// Wire shape of a single window over D-Bus. Mirrors the dict the extension
/// builds in `extension/gnome/src/windows.ts::serialize`. We only decode the
/// fields lofi-core's `Window` needs; the extension's other fields
/// (`monitor`, geometry, etc.) are ignored by zvariant's dict decoder.
#[derive(Debug, Type, DeserializeDict)]
#[zvariant(signature = "a{sv}")]
struct DbusWindow {
    id: u64,
    title: String,
    app_name: String,
    icon: String,
    workspace: i32,
}

/// Open a fresh session-bus connection. Kept private because every public
/// entry point needs its own, and we don't share connections across calls
/// (the launcher is short-lived enough that connection reuse buys nothing).
fn connect() -> zbus::Result<Connection> {
    Connection::session()
}

/// Coerce empty strings to `None` so a window with no associated Shell.App
/// (system surfaces, override-redirect children) renders without a phantom
/// icon or app-name suffix.
fn map_dbus_window(w: DbusWindow) -> Window {
    let app_name = if w.app_name.is_empty() {
        None
    } else {
        Some(w.app_name)
    };
    let icon = if w.icon.is_empty() {
        None
    } else {
        Some(w.icon)
    };
    Window {
        id: w.id,
        title: w.title,
        app_name,
        icon,
        workspace: w.workspace,
    }
}

/// Ask the extension for the current window list. Any D-Bus failure
/// (connection refused, no name owner, malformed reply) yields an empty Vec
/// after an `eprintln!`. The launcher then shows only Application entries —
/// degraded but functional.
pub fn gather_windows() -> Vec<Window> {
    let connection = match connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lofi: connect to session bus failed: {e}");
            return Vec::new();
        }
    };

    let proxy = match WindowManagerProxy::new(&connection) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lofi: create WindowManager proxy failed: {e}");
            return Vec::new();
        }
    };

    let raw = match proxy.list_windows() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lofi: list_windows failed: {e}");
            return Vec::new();
        }
    };

    raw.into_iter().map(map_dbus_window).collect()
}

/// Ask the extension to raise the window with `id`. Errors are logged and
/// swallowed — there's no caller-side recovery from "the window vanished
/// between gather and click".
pub fn focus_window(id: u64) {
    let connection = match connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lofi: connect to session bus failed: {e}");
            return;
        }
    };

    let proxy = match WindowManagerProxy::new(&connection) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lofi: create WindowManager proxy failed: {e}");
            return;
        }
    };

    if let Err(e) = proxy.focus_window(id) {
        eprintln!("lofi: focus window {id} failed: {e}");
    }
}
