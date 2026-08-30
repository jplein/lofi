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

use lofi_core::{Window, WorkArea};
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

    /// Same shape as `list_windows`, but ordered most-recently-focused first
    /// (the order Alt+Tab cycles through). Backed by Mutter's per-display
    /// tab list, which is the canonical MRU source.
    ///
    /// The extension exports this method as `ListWindowsMRU` (capital MRU
    /// acronym). heck's PascalCase conversion would produce `ListWindowsMru`,
    /// so we override the wire name explicitly to match the service.
    #[zbus(name = "ListWindowsMRU")]
    fn list_windows_mru(&self) -> zbus::Result<Vec<DbusWindow>>;

    /// Raise the window with `id` and switch to its workspace.
    fn focus_window(&self, id: u64) -> zbus::Result<()>;

    /// Move the window with `id` to the workspace at the 0-based
    /// `target_index`. The extension unsticks an on-all-workspaces window
    /// first and (under dynamic workspaces) appends a workspace when the
    /// target is one past the end; LoFi only ever passes indices of
    /// already-open workspaces, so neither path fires in practice.
    fn move_window_to_workspace(&self, id: u64, target_index: i32) -> zbus::Result<()>;

    /// Minimize the window with `id`.
    fn minimize_window(&self, id: u64) -> zbus::Result<()>;

    /// Toggle the maximized state of the window with `id`. Toggle state is
    /// resolved on the extension side because Mutter holds the live state
    /// and a Rust-side capture-then-act would race against external changes.
    fn toggle_maximize_window(&self, id: u64) -> zbus::Result<()>;

    /// Toggle the fullscreen state of the window with `id`. Same rationale
    /// as `toggle_maximize_window` for resolving on the extension side.
    fn toggle_fullscreen_window(&self, id: u64) -> zbus::Result<()>;

    /// Move and resize the window with `id` to the given frame rectangle.
    /// The extension unmaximizes and unfullscreens the window first because
    /// `move_resize_frame` is a no-op on a maximized/fullscreen window.
    fn move_resize_window(
        &self,
        id: u64,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> zbus::Result<()>;

    /// Return the work area (monitor rectangle minus panel/dock struts) for
    /// the monitor that owns the window with `id`. Used by the geometry
    /// commands as the bounding box for the target rectangle.
    fn get_window_work_area(&self, id: u64) -> zbus::Result<DbusWorkArea>;

    /// Return the current frame rectangle (`x`, `y`, `width`, `height`) of
    /// the window with `id`. Only the geometry-preserving `Center` command
    /// reads this; the other geometry commands compute purely from the work
    /// area.
    fn get_window_frame(&self, id: u64) -> zbus::Result<DbusFrame>;
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
    app_desktop_id: String,
    icon: String,
    workspace: i32,
}

/// Wire shape of a monitor work area dict returned by `GetWindowWorkArea`.
/// `a{sv}` of four signed-int variants — same shape the extension emits in
/// `service.ts::GetWindowWorkArea`.
#[derive(Debug, Type, DeserializeDict)]
#[zvariant(signature = "a{sv}")]
struct DbusWorkArea {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// Wire shape of a window frame dict returned by `GetWindowFrame`. Same
/// shape as `DbusWorkArea` but kept as a distinct type so the public
/// wrappers can return semantically-correct tuple/struct types.
#[derive(Debug, Type, DeserializeDict)]
#[zvariant(signature = "a{sv}")]
struct DbusFrame {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
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
    let app_desktop_id = if w.app_desktop_id.is_empty() {
        None
    } else {
        Some(w.app_desktop_id)
    };
    Window {
        id: w.id,
        title: w.title,
        app_name,
        icon,
        workspace: w.workspace,
        app_desktop_id,
    }
}

/// Ask the extension for the current window list in MRU order, most recent
/// first (the order Alt+Tab cycles through). Any D-Bus failure (connection
/// refused, no name owner, malformed reply) yields an empty Vec after an
/// `eprintln!`. The launcher then shows only Application entries — degraded
/// but functional.
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

    let raw = match proxy.list_windows_mru() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lofi: list_windows_mru failed: {e}");
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

/// Ask the extension to move the window with `id` to the workspace at the
/// 0-based `target_index`. Same log-and-swallow degradation as `focus_window`:
/// there's no caller-side recovery from "the window or workspace vanished
/// between gather and click".
pub fn move_window_to_workspace(id: u64, target_index: i32) {
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

    if let Err(e) = proxy.move_window_to_workspace(id, target_index) {
        eprintln!("lofi: move_window_to_workspace {id} -> {target_index} failed: {e}");
    }
}

/// Ask the extension to minimize the window with `id`. Same log-and-swallow
/// degradation as `focus_window`.
pub fn minimize_window(id: u64) {
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

    if let Err(e) = proxy.minimize_window(id) {
        eprintln!("lofi: minimize_window {id} failed: {e}");
    }
}

/// Ask the extension to toggle the maximized state of the window with `id`.
pub fn toggle_maximize_window(id: u64) {
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

    if let Err(e) = proxy.toggle_maximize_window(id) {
        eprintln!("lofi: toggle_maximize_window {id} failed: {e}");
    }
}

/// Ask the extension to toggle the fullscreen state of the window with `id`.
pub fn toggle_fullscreen_window(id: u64) {
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

    if let Err(e) = proxy.toggle_fullscreen_window(id) {
        eprintln!("lofi: toggle_fullscreen_window {id} failed: {e}");
    }
}

/// Ask the extension to move and resize the window with `id` to the given
/// frame rectangle. The extension unmaximizes / unfullscreens first.
pub fn move_resize_window(id: u64, x: i32, y: i32, width: i32, height: i32) {
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

    if let Err(e) = proxy.move_resize_window(id, x, y, width, height) {
        eprintln!("lofi: move_resize_window {id} failed: {e}");
    }
}

/// Ask the extension for the work area of the monitor that owns the window
/// with `id`. Returns `None` on any D-Bus failure (connection, proxy, or
/// missing window) so the caller can degrade — `commands::gather_commands`
/// drops the entire command set when the work area can't be read.
pub fn get_window_work_area(id: u64) -> Option<WorkArea> {
    let connection = match connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lofi: connect to session bus failed: {e}");
            return None;
        }
    };

    let proxy = match WindowManagerProxy::new(&connection) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lofi: create WindowManager proxy failed: {e}");
            return None;
        }
    };

    match proxy.get_window_work_area(id) {
        Ok(wa) => Some(WorkArea {
            x: wa.x,
            y: wa.y,
            width: wa.width,
            height: wa.height,
        }),
        Err(e) => {
            eprintln!("lofi: get_window_work_area {id} failed: {e}");
            None
        }
    }
}

/// Ask the extension for the current frame rectangle of the window with
/// `id`. Returns `None` on any D-Bus failure. Used only by the `Center`
/// command, which keeps the window's current size while recentering.
pub fn get_window_frame(id: u64) -> Option<(i32, i32, i32, i32)> {
    let connection = match connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lofi: connect to session bus failed: {e}");
            return None;
        }
    };

    let proxy = match WindowManagerProxy::new(&connection) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lofi: create WindowManager proxy failed: {e}");
            return None;
        }
    };

    match proxy.get_window_frame(id) {
        Ok(f) => Some((f.x, f.y, f.width, f.height)),
        Err(e) => {
            eprintln!("lofi: get_window_frame {id} failed: {e}");
            None
        }
    }
}
