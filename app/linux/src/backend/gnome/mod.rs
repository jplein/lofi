//! The GNOME backend: a blocking D-Bus client for the LoFi GNOME Shell
//! extension (`extension/gnome/`), which publishes
//! `dev.jplein.LoFi.Shell.WindowManager` on the session bus.
//!
//! The extension is required because Wayland clients can't enumerate or
//! manipulate other apps' windows directly and Mutter doesn't implement
//! `wlr-foreign-toplevel-management`; `org.gnome.Shell.Introspect` exists but
//! is read-only and too narrow (no workspace assignment, no per-window
//! geometry) to drive the launcher. See `app/linux/README.md`.
//!
//! The submodules here are the original per-concern clients, unchanged in
//! substance: [`windows`] (listing, focus, geometry, state toggles),
//! [`workspaces`] (listing, switch), [`commands`] (gathering the window-action
//! and workspace-move command sets), and [`power`] (session/logind power
//! commands). [`GnomeBackend`] is the thin adapter that presents them as the
//! crate's [`Backend`](super::Backend) trait; it holds no state, because every
//! one of those functions opens its own short-lived session-bus connection.

pub mod commands;
pub mod power;
pub mod windows;
pub mod workspaces;

use lofi_core::{
    Command, CommandKind, PowerCommand, PowerCommandKind, SummonWindow, Window, Workspace,
    WorkspaceCommand, compute_geometry,
};

use super::Backend;

/// GNOME implementation of [`Backend`]. Stateless: each call opens its own
/// blocking session-bus connection (see `windows::connect`), so there is
/// nothing to cache between the gather pass and the activation call.
pub struct GnomeBackend;

impl GnomeBackend {
    pub fn new() -> Self {
        GnomeBackend
    }
}

impl Default for GnomeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for GnomeBackend {
    fn name(&self) -> &'static str {
        "gnome"
    }

    fn gather_windows(&self) -> Vec<Window> {
        windows::gather_windows()
    }

    fn gather_workspaces(&self) -> Vec<Workspace> {
        workspaces::gather_workspaces()
    }

    fn gather_commands(&self, windows: &[Window]) -> Vec<Command> {
        commands::gather_commands(windows)
    }

    fn gather_workspace_commands(
        &self,
        windows: &[Window],
        workspaces: &[Workspace],
    ) -> Vec<WorkspaceCommand> {
        commands::gather_workspace_commands(windows, workspaces)
    }

    fn gather_power_commands(&self) -> Vec<PowerCommand> {
        power::gather_power_commands()
    }

    /// Always empty: "Summon window" puts a window in the column right of
    /// another, and GNOME floats every window, so there are no columns.
    fn gather_summon_commands(&self, _windows: &[Window]) -> Vec<SummonWindow> {
        Vec::new()
    }

    fn focus_window(&self, id: u64) {
        windows::focus_window(id);
    }

    fn activate_workspace(&self, index: i32) {
        workspaces::activate_workspace(index);
    }

    /// Dispatch a window-action command.
    ///
    /// The three state-toggle kinds route to dedicated by-id methods on the
    /// extension; the geometry kinds run through `lofi_core::compute_geometry`
    /// and land as a single `MoveResizeWindow`. Resolving the toggle state on
    /// the extension side (rather than reading it here and acting on the
    /// result) is deliberate: Mutter holds the live state, and a Rust-side
    /// capture-then-act would race against an external change.
    ///
    /// The Niri-only kinds are unreachable here — `commands::ALL_KINDS` never
    /// emits them on GNOME — but the match is exhaustive rather than using a
    /// `_` arm so that adding a `CommandKind` variant forces a decision here
    /// instead of silently becoming a no-op.
    fn run_command(&self, command: &Command) {
        let id = command.target_window_id;
        match command.kind {
            CommandKind::Minimize => windows::minimize_window(id),
            CommandKind::ToggleMaximize => windows::toggle_maximize_window(id),
            CommandKind::ToggleFullscreen => windows::toggle_fullscreen_window(id),
            CommandKind::Center
            | CommandKind::CenterThird
            | CommandKind::CenterHalf
            | CommandKind::CenterTwoThirds
            | CommandKind::LeftThird
            | CommandKind::LeftHalf
            | CommandKind::LeftTwoThirds
            | CommandKind::RightThird
            | CommandKind::RightHalf
            | CommandKind::RightTwoThirds
            | CommandKind::StandardSize => {
                if let Some((x, y, w, h)) =
                    compute_geometry(command.kind, &command.work_area, command.current_frame)
                {
                    windows::move_resize_window(id, x, y, w, h);
                }
            }
            // macOS-only (`NextDisplay` / `PreviousDisplay`) and Niri-only
            // kinds. Neither appears in this backend's gathered set, so
            // reaching them would mean a gather/dispatch mismatch — log it
            // rather than failing silently.
            other => eprintln!(
                "gnome: no dispatch for command kind {:?}; ignoring",
                other.as_id()
            ),
        }
    }

    /// Move the target window to `target_index`, then switch to that
    /// workspace so the user follows the window they just moved rather than
    /// being left behind on the source workspace.
    ///
    /// Two sequential blocking calls: the move lands before the switch
    /// because both block, and each logs-and-degrades independently (a failed
    /// move still attempts the switch). The dispatch does not branch on
    /// `command.kind` — `target_index` is the already-resolved destination for
    /// every flavour, absolute or relative (see
    /// `lofi_core::build_workspace_commands`).
    fn run_workspace_command(&self, command: &WorkspaceCommand) {
        windows::move_window_to_workspace(command.target_window_id, command.target_index);
        workspaces::activate_workspace(command.target_index);
    }

    fn run_power_command(&self, kind: PowerCommandKind) {
        power::activate(kind);
    }

    /// Unreachable in practice — `gather_summon_commands` emits nothing — but
    /// logged rather than silent, like the Niri backend's unhandled kinds.
    fn run_summon_command(&self, summon: &SummonWindow) {
        eprintln!(
            "gnome: summon window {} is not supported; ignoring",
            summon.window.id
        );
    }

    /// False: Mutter does not implement `wlr-layer-shell`, so the launcher is
    /// an ordinary toplevel here — which is the right presentation on GNOME
    /// anyway, since GNOME floats every window.
    fn uses_layer_shell(&self) -> bool {
        false
    }
}
