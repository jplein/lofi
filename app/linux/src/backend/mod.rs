//! Desktop backends: the seam between the desktop-agnostic launcher (entry
//! gathering, ranking, UI, MRU) and the window-system-specific plumbing that
//! actually enumerates and manipulates windows and workspaces.
//!
//! Two implementations live here:
//!
//! - [`gnome`] — talks to the LoFi GNOME Shell extension over the session
//!   bus (`dev.jplein.LoFi.Shell.WindowManager`). See `extension/gnome/`.
//! - [`niri`] — talks to the running Niri compositor over its own
//!   newline-delimited-JSON unix socket at `$NIRI_SOCKET`.
//!
//! One `lofi` binary serves both; the choice is made at run time by
//! [`detect`] rather than at build time by a Cargo feature. A feature flag
//! would mean two binaries, two Nix packages, and a home-manager option the
//! user has to keep in sync with the session they actually log into — for a
//! launcher that costs nothing to link both ways, run-time detection is the
//! cheaper and less error-prone answer.
//!
//! Every backend method follows the same error policy as the code it grew
//! out of: log to stderr and degrade (empty `Vec`, or a silent no-op for the
//! action methods). Nothing here panics, unwraps, or expects. The launcher
//! window has already closed by the time an action lands, so there is no UI
//! surface to report a failure through, and a launcher that refuses to open
//! because the compositor did not answer is worse than one that opens with
//! the application list alone.

pub mod gnome;
pub mod logind;
pub mod niri;

use std::env;
use std::rc::Rc;

use lofi_core::{
    Command, PowerCommand, PowerCommandKind, SummonWindow, Window, Workspace, WorkspaceCommand,
};

use crate::config::Config;

/// Canonical `.desktop` id of the launcher itself. Both backends compare
/// against it to skip LoFi's own window when picking the target for the
/// window-action and workspace-move commands — otherwise every one of those
/// commands would resize, move, or close the launcher.
///
/// Shared rather than duplicated per backend because it is one fact about
/// this binary, not about either desktop: it must stay in lockstep with
/// `APP_ID` in `main.rs` and with the id of the installed `.desktop` file.
pub const LOFI_DESKTOP_ID: &str = "dev.jplein.LoFi.desktop";

/// Which desktop the launcher is running under. Kept as a separate enum from
/// the `Backend` trait object so `detect` can be unit-tested against a plain
/// environment snapshot without constructing a live D-Bus or socket client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Niri,
}

/// Decide which backend this session wants.
///
/// `niri_socket` is `$NIRI_SOCKET` and `xdg_current_desktop` is
/// `$XDG_CURRENT_DESKTOP`; both are passed in rather than read from the
/// process environment so this stays a pure function (the same reason
/// `apps::gather_applications` takes its directories as a parameter).
///
/// `$NIRI_SOCKET` is the primary signal because it is the thing the backend
/// actually needs — Niri exports it into every process it spawns, and its
/// presence means there is a socket to talk to. `$XDG_CURRENT_DESKTOP` is
/// only a fallback for the case where the launcher was started from outside
/// the compositor's own environment (a systemd user unit that didn't import
/// it, an `ssh` shell): we would rather pick the Niri backend and log a
/// missing-socket error than silently pick GNOME and time out on a D-Bus
/// name that will never be owned.
///
/// Anything else falls through to GNOME. That is the historical default and
/// keeps existing installs working with no configuration.
pub fn detect(niri_socket: Option<&str>, xdg_current_desktop: Option<&str>) -> Desktop {
    if niri_socket.is_some_and(|s| !s.is_empty()) {
        return Desktop::Niri;
    }
    // XDG_CURRENT_DESKTOP is a colon-separated list ("niri", but also e.g.
    // "GNOME:GNOME-Classic"), so match per component rather than on the
    // whole string.
    if xdg_current_desktop
        .is_some_and(|v| v.split(':').any(|part| part.eq_ignore_ascii_case("niri")))
    {
        return Desktop::Niri;
    }
    Desktop::Gnome
}

/// Build the backend for the current session. Reads the environment once and
/// hands the decision to [`detect`].
///
/// Returns an `Rc` because the backend outlives `main`'s gather step: the UI
/// closures hold it so `launch::activate` can dispatch on Enter or click,
/// and the Niri backend caches its workspace table across the two (see
/// `niri::NiriBackend`).
///
/// `config` is consulted for exactly one thing today — the Niri backend's
/// `lock-command`, which it copies out rather than borrowing because the
/// backend outlives the gather step that owns the `Config`. The GNOME backend
/// ignores it: its Lock goes through `org.gnome.ScreenSaver`, which needs no
/// locker to be named. The whole `Config` is passed rather than that one field
/// so a second setting does not change this signature again.
pub fn create(config: &Config) -> Rc<dyn Backend> {
    let niri_socket = env::var("NIRI_SOCKET").ok();
    let xdg = env::var("XDG_CURRENT_DESKTOP").ok();
    match detect(niri_socket.as_deref(), xdg.as_deref()) {
        Desktop::Niri => Rc::new(niri::NiriBackend::new(config.lock_command.clone())),
        Desktop::Gnome => Rc::new(gnome::GnomeBackend::new()),
    }
}

/// The window-system surface the launcher needs from a desktop.
///
/// Split into a gather half (called once at startup, before the window is
/// shown) and an act half (called once, from the UI closure, immediately
/// before the window closes). Nothing in between re-reads: the launcher is a
/// short-lived process and a snapshot taken at startup is what the user is
/// looking at when they press Enter.
///
/// The act half is deliberately coarse — `run_command` takes a whole
/// `Command` rather than exposing `move_resize_window` / `minimize_window` /
/// … as trait methods. The two desktops disagree about what a window action
/// *is*: GNOME computes a rectangle with `lofi_core::compute_geometry` and
/// sends one `MoveResizeWindow`, while Niri has no free-form geometry for
/// tiled windows and sends a proportional column-width action instead. A
/// per-primitive trait would force one desktop's vocabulary onto the other;
/// handing over the `Command` lets each backend own its own mapping. The
/// same reasoning applies to `run_workspace_command`, which GNOME performs
/// as two calls (move, then switch) and Niri as one.
pub trait Backend {
    /// Short name for diagnostics (`"gnome"` / `"niri"`).
    fn name(&self) -> &'static str;

    /// Open windows, most-recently-focused first. The MRU order is load
    /// bearing: `main`'s app-to-recent-window combine step takes the *first*
    /// occurrence of each `app_desktop_id`, and `gather_commands` /
    /// `gather_workspace_commands` take the first non-LoFi entry as their
    /// target window.
    fn gather_windows(&self) -> Vec<Window>;

    /// Open workspaces in display order. `Workspace::index` is 0-based and
    /// dense over the returned slice; it is what `EntryRef::Workspace` and
    /// the workspace-move command ids key on, and what `activate_workspace`
    /// and `run_workspace_command` are handed back.
    fn gather_workspaces(&self) -> Vec<Workspace>;

    /// Window-action commands for the target window, or an empty `Vec` when
    /// there is no usable target. `windows` is the already-gathered MRU list
    /// so the backend does not have to enumerate a second time.
    fn gather_commands(&self, windows: &[Window]) -> Vec<Command>;

    /// Workspace-move commands for the target window, or an empty `Vec` when
    /// there is no usable target.
    fn gather_workspace_commands(
        &self,
        windows: &[Window],
        workspaces: &[Workspace],
    ) -> Vec<WorkspaceCommand>;

    /// Power commands this desktop can actually perform.
    fn gather_power_commands(&self) -> Vec<PowerCommand>;

    /// "Summon window" rows: one per other window that can be brought to the
    /// right of the target window. Empty on desktops without a column layout
    /// to summon into (GNOME).
    fn gather_summon_commands(&self, windows: &[Window]) -> Vec<SummonWindow>;

    /// Raise the window with `id`, switching workspace if needed.
    fn focus_window(&self, id: u64);

    /// Switch to the workspace at the 0-based `index` from
    /// `gather_workspaces`.
    fn activate_workspace(&self, index: i32);

    /// Perform a window-action command. The target window, and whatever
    /// geometry context the backend baked in at gather time, both travel on
    /// the `Command`.
    fn run_command(&self, command: &Command);

    /// Move the target window to `command.target_index` and follow it there.
    fn run_workspace_command(&self, command: &WorkspaceCommand);

    /// Perform a power command.
    fn run_power_command(&self, kind: PowerCommandKind);

    /// Bring `summon.window` into the column directly right of
    /// `summon.target_window_id`, and focus it.
    fn run_summon_command(&self, summon: &SummonWindow);

    /// Whether the launcher window should be presented as a `wlr-layer-shell`
    /// overlay surface rather than an ordinary toplevel. True on Niri, where
    /// an ordinary toplevel would be tiled into the scrolling layout; false
    /// on GNOME, where Mutter does not implement the protocol at all. See
    /// `ui::build`.
    fn uses_layer_shell(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::{Desktop, detect};

    #[test]
    fn niri_socket_selects_niri() {
        assert_eq!(
            detect(Some("/run/user/1000/niri.wayland-1.4193.sock"), None),
            Desktop::Niri,
            "a non-empty NIRI_SOCKET must select the Niri backend"
        );
    }

    #[test]
    fn niri_socket_wins_over_xdg_current_desktop() {
        // A stale or inherited XDG_CURRENT_DESKTOP must not override the
        // socket, which is the thing the backend actually needs.
        assert_eq!(
            detect(Some("/run/user/1000/niri.sock"), Some("GNOME")),
            Desktop::Niri,
            "NIRI_SOCKET must take precedence over XDG_CURRENT_DESKTOP"
        );
    }

    #[test]
    fn empty_niri_socket_is_not_a_signal() {
        // An exported-but-empty variable is the shell's way of saying
        // "unset"; treating it as present would send us to a socket path of
        // "" that can never connect.
        assert_eq!(
            detect(Some(""), Some("GNOME")),
            Desktop::Gnome,
            "an empty NIRI_SOCKET must not select Niri"
        );
        assert_eq!(
            detect(Some(""), Some("niri")),
            Desktop::Niri,
            "an empty NIRI_SOCKET must still fall through to the XDG check"
        );
    }

    #[test]
    fn xdg_current_desktop_selects_niri_case_insensitively() {
        for value in ["niri", "Niri", "NIRI"] {
            assert_eq!(
                detect(None, Some(value)),
                Desktop::Niri,
                "XDG_CURRENT_DESKTOP={value:?} should select Niri"
            );
        }
    }

    #[test]
    fn xdg_current_desktop_matches_per_component() {
        // The variable is a colon-separated list; a component match is the
        // spec-defined semantics.
        assert_eq!(
            detect(None, Some("niri:wlroots")),
            Desktop::Niri,
            "Niri as the first component should match"
        );
        assert_eq!(
            detect(None, Some("wlroots:niri")),
            Desktop::Niri,
            "Niri as a later component should match"
        );
        // A substring that is not its own component must not match, or
        // something like "niriwm" or "GNOME-niri-shim" would misroute.
        assert_eq!(
            detect(None, Some("niriwm")),
            Desktop::Gnome,
            "a substring match must not select Niri"
        );
    }

    #[test]
    fn unset_environment_falls_back_to_gnome() {
        assert_eq!(
            detect(None, None),
            Desktop::Gnome,
            "GNOME is the default when nothing points at Niri"
        );
        assert_eq!(
            detect(None, Some("GNOME")),
            Desktop::Gnome,
            "XDG_CURRENT_DESKTOP=GNOME should select GNOME"
        );
    }
}
