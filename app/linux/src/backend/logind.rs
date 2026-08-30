//! systemd-logind power calls shared by both backends.
//!
//! logind lives on the **system** bus, not the session bus, and is present in
//! any systemd session regardless of desktop — which is exactly why these
//! three calls are shared rather than duplicated per backend. GNOME uses only
//! `suspend` from here (its Restart/Shutdown route through
//! `org.gnome.SessionManager` so the shell's confirmation dialog fires); Niri
//! has no session manager of its own, so it uses all three.
//!
//! Every call passes `interactive = false`. The bool is logind's polkit-prompt
//! flag: `true` lets logind raise an authentication dialog, `false` fails the
//! call outright if the caller isn't already authorised. `false` is right here
//! because the launcher window has already closed by the time the call lands,
//! so there is nothing to parent a prompt to — and on a normal desktop the
//! active local user is permitted all three actions without one.
//!
//! Like `power.rs`, this uses the lower-level `zbus::blocking::Proxy` rather
//! than a generated `#[zbus::proxy]` trait: three one-line calls against one
//! interface don't earn a generated trait.

use zbus::blocking::{Connection, Proxy};

const SERVICE: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";

/// Open the system-bus `login1.Manager` proxy. Private because every caller
/// wants one of the three verbs below, not the proxy itself.
fn manager(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE, PATH, MANAGER)
}

/// Suspend to RAM.
pub fn suspend() -> zbus::Result<()> {
    let conn = Connection::system()?;
    manager(&conn)?.call_method("Suspend", &(false,))?;
    Ok(())
}

/// Reboot.
pub fn reboot() -> zbus::Result<()> {
    let conn = Connection::system()?;
    manager(&conn)?.call_method("Reboot", &(false,))?;
    Ok(())
}

/// Power off.
pub fn power_off() -> zbus::Result<()> {
    let conn = Connection::system()?;
    manager(&conn)?.call_method("PowerOff", &(false,))?;
    Ok(())
}

/// Ask logind to lock the current session — i.e. emit the `Lock` signal on
/// this session's object for a lock handler to act on.
///
/// This is a *request*, not a lock: it only does anything if some daemon
/// (`swayidle`, `hypridle`, `xss-lock`, a desktop's own screensaver) is
/// listening for the signal. On a session with no such daemon it succeeds and
/// nothing happens, which is why the Niri backend treats it as the last
/// resort in its lock chain rather than the default (see `niri::power`).
///
/// `/org/freedesktop/login1/session/auto` is logind's alias for "the calling
/// process's own session", so no session-id lookup is needed.
pub fn lock_session() -> zbus::Result<()> {
    let conn = Connection::system()?;
    let proxy = Proxy::new(
        &conn,
        SERVICE,
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
    )?;
    proxy.call_method("Lock", &())?;
    Ok(())
}
