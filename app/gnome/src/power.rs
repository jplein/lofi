//! System-level power commands: Lock, Suspend, Restart, Shutdown.
//!
//! Each command is a one-shot D-Bus method call against an existing GNOME
//! or systemd-logind service. Lock/Restart/Shutdown route through the
//! session bus (GNOME's `ScreenSaver` and `SessionManager`); Suspend goes
//! to the SYSTEM bus's `org.freedesktop.login1.Manager` because there's no
//! GNOME-level Suspend wrapper.
//!
//! Restart and Shutdown go through `org.gnome.SessionManager.Reboot()` and
//! `Shutdown()` (rather than logind's direct equivalents) on purpose: those
//! methods raise GNOME's standard 60-second confirmation dialog, matching
//! the system-menu behaviour and protecting against accidental triggers.
//! Lock uses `org.gnome.ScreenSaver.Lock`. Suspend uses logind's
//! `Suspend(false)` (the bool is `interactive`; `false` skips the polkit
//! prompt — suspend is almost always allowed for active users).
//!
//! We use the lower-level `zbus::blocking::Proxy::call_method` rather than
//! generating four `#[zbus::proxy]` traits — each call is one line and a
//! generated trait would be more noise than signal here.

use lofi_core::{PowerCommand, PowerCommandKind};
use zbus::blocking::{Connection, Proxy};

/// Full set of power-command kinds. Mirrors the `ALL_POWER_COMMAND_KINDS`
/// constant in `lofi-core`'s tests; kept here because `gather_power_commands`
/// needs to enumerate them and importing test fixtures from another crate
/// would be backwards.
const ALL_KINDS: &[PowerCommandKind] = &[
    PowerCommandKind::LockSession,
    PowerCommandKind::Suspend,
    PowerCommandKind::Restart,
    PowerCommandKind::Shutdown,
];

/// Static set of power commands. Always returned in full — they don't depend
/// on the focused window or any runtime state, so there's no gather guard.
pub fn gather_power_commands() -> Vec<PowerCommand> {
    ALL_KINDS
        .iter()
        .map(|&kind| PowerCommand { kind })
        .collect()
}

/// Dispatch a power command via D-Bus. Logs and returns on failure; never
/// panics. Matches the rest of the codebase's `eprintln!`-and-degrade policy
/// — there's no meaningful caller-side recovery from a transient D-Bus error
/// and the launcher window has already closed.
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
    let proxy = Proxy::new(
        &conn,
        "org.gnome.ScreenSaver",
        "/org/gnome/ScreenSaver",
        "org.gnome.ScreenSaver",
    )?;
    proxy.call_method("Lock", &())?;
    Ok(())
}

fn suspend() -> zbus::Result<()> {
    // Suspend lives on the SYSTEM bus, not session. `interactive=false` skips
    // the polkit prompt — suspend is almost always allowed for active users.
    let conn = Connection::system()?;
    let proxy = Proxy::new(
        &conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )?;
    proxy.call_method("Suspend", &(false,))?;
    Ok(())
}

fn restart() -> zbus::Result<()> {
    // Routed through GNOME's SessionManager (not logind) so the standard
    // 60-second restart-confirmation dialog fires.
    let conn = Connection::session()?;
    let proxy = Proxy::new(
        &conn,
        "org.gnome.SessionManager",
        "/org/gnome/SessionManager",
        "org.gnome.SessionManager",
    )?;
    proxy.call_method("Reboot", &())?;
    Ok(())
}

fn shutdown() -> zbus::Result<()> {
    // Same as `restart` — routed through SessionManager for the confirmation
    // dialog instead of logind's direct PowerOff().
    let conn = Connection::session()?;
    let proxy = Proxy::new(
        &conn,
        "org.gnome.SessionManager",
        "/org/gnome/SessionManager",
        "org.gnome.SessionManager",
    )?;
    proxy.call_method("Shutdown", &())?;
    Ok(())
}
