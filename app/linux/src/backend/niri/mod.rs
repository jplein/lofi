//! The Niri backend: a client for the compositor's own IPC socket.
//!
//! Niri needs no LoFi-side extension the way GNOME does — it ships a complete
//! window/workspace IPC surface in the compositor itself, reachable at
//! `$NIRI_SOCKET`. [`ipc`] speaks that protocol; [`appid`] does the one piece
//! of enrichment Niri does not do for us (mapping a Wayland `app_id` to a
//! desktop entry); [`power`] handles the power commands, which have no Niri
//! equivalent of GNOME's session manager.
//!
//! ## What is different from GNOME
//!
//! **Niri does not order windows by recency.** `Windows` comes back in
//! unspecified order, so [`NiriBackend::gather_windows`] sorts on each
//! window's `focus_timestamp` (see `ipc::NiriWindow::sort_key`). Everything
//! downstream — the app-to-recent-window combine step in `main`, the target
//! pick for the command sets — depends on that order, exactly as it depends on
//! GNOME's `ListWindowsMRU`.
//!
//! **Niri's workspace ids are not durable.** Under dynamic workspaces the
//! trailing empty workspace on each output is destroyed and recreated with a
//! fresh id as focus moves away from it, so a persisted `EntryRef::Workspace`
//! keyed on a Niri id would rot within one session. LoFi therefore keys on the
//! workspace's *position* in the sorted table and translates back to an id at
//! activation time — see [`NiriBackend::workspace_table`].
//!
//! **Niri is a scrolling tiler, so there is no free-form window geometry.** A
//! tiled window's position is a consequence of its place in the scroll order,
//! not something a client sets, and its height is the column's. The eleven
//! `compute_geometry` command kinds therefore have no meaning here and are not
//! emitted; the kinds this backend emits are proportional column widths and
//! state toggles. `Minimize` is absent for the same reason — Niri has no
//! minimized state at all. See [`ALL_KINDS`].

pub mod appid;
pub mod ipc;
pub mod power;

use std::cell::RefCell;
use std::collections::HashMap;

use lofi_core::{
    Command, CommandKind, PowerCommand, PowerCommandKind, Window, WorkArea, Workspace,
    WorkspaceCommand, WorkspaceCommandKind, build_workspace_commands,
};

use super::{Backend, LOFI_DESKTOP_ID};
use appid::AppIdResolver;
use ipc::{Action, SizeChange, WorkspaceReference};

/// Window-action command kinds this backend emits, in list order.
///
/// Deliberately *not* the GNOME set. The eleven geometry kinds
/// (`LeftHalf`, `RightThird`, …) describe a position and size inside a work
/// area, which is not a thing a window has under a scrolling tiler: columns
/// are laid out by scroll order and fill the height. What a Niri window does
/// have is a proportional column width, so those rows are width presets. The
/// rest are state toggles Niri does implement.
///
/// `Minimize` is absent because Niri has no minimized state — there is nowhere
/// for a window to go. The `*Display` kinds are absent for the same reason
/// they are on GNOME: they are the macOS frontend's.
const ALL_KINDS: &[CommandKind] = &[
    CommandKind::Center,
    CommandKind::WidthThird,
    CommandKind::WidthHalf,
    CommandKind::WidthTwoThirds,
    CommandKind::MaximizeColumn,
    CommandKind::ExpandColumn,
    CommandKind::ToggleFloating,
    CommandKind::ToggleMaximize,
    CommandKind::ToggleFullscreen,
    CommandKind::Close,
];

/// Sentinel `Window::workspace` for a window whose workspace LoFi could not
/// place in its table. Mirrors the negative index GNOME reports for a sticky
/// (on-all-workspaces) window, and `lofi_core::build_workspace_commands`
/// already treats a negative index as "no single current workspace": the
/// absolute moves are still offered, the relative prev/next ones are not.
const UNKNOWN_WORKSPACE: i32 = -1;

/// One row of the workspace table: LoFi's stable positional index paired with
/// the Niri id the IPC actually wants.
#[derive(Debug, Clone)]
struct WorkspaceRow {
    /// Niri's own id. Used for every action; never persisted.
    niri_id: u64,
    /// 0-based position in the sorted table. This is `Workspace::index`, the
    /// payload of `EntryRef::Workspace`, and therefore the persistent MRU key.
    index: i32,
    /// Display label. Not derived from `index`: see [`workspace_label`].
    name: String,
    /// Connector name of the output this workspace lives on. Carried here so
    /// `work_area_for` can go window → workspace → output without a second
    /// `Workspaces` request.
    output: Option<String>,
}

/// Niri implementation of [`Backend`].
///
/// Holds two caches, both scoped to the single launcher invocation that owns
/// this backend:
///
/// - `workspaces` — the position↔id table, built on first use. It has to be
///   shared rather than rebuilt per call because three separate things need
///   the *same* mapping: `gather_workspaces` (which hands out the positions),
///   `gather_windows` (which translates each window's `workspace_id` into
///   one), and `activate_workspace` / `run_workspace_command` (which translate
///   back). Rebuilding between them would let a workspace created or destroyed
///   mid-gather shift every index.
/// - `apps` — the `app_id` → desktop-entry resolver's lazy `StartupWMClass`
///   index (see `appid`).
pub struct NiriBackend {
    workspaces: RefCell<Option<Vec<WorkspaceRow>>>,
    apps: AppIdResolver,
}

impl NiriBackend {
    pub fn new() -> Self {
        NiriBackend {
            workspaces: RefCell::new(None),
            apps: AppIdResolver::new(),
        }
    }

    /// The workspace table, fetched on first use and cached thereafter.
    ///
    /// Sorted by output connector name, then by Niri's per-output `idx`, and
    /// positions assigned over that order. Sorting by output name is what
    /// makes the ordering *stable*: Niri's `Workspaces` response is not
    /// ordered, and an unstable order would mean a persisted "workspace 2" MRU
    /// row pointing at a different workspace on the next launch — the exact
    /// unpredictability LoFi is trying to avoid.
    ///
    /// Returns an empty table on any IPC failure, which degrades the launcher
    /// to no workspace rows and no workspace-move rows rather than failing to
    /// open.
    fn workspace_table(&self) -> Vec<WorkspaceRow> {
        if let Some(cached) = self.workspaces.borrow().as_ref() {
            return cached.clone();
        }

        let mut raw = match ipc::workspaces() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("lofi: niri list workspaces failed: {e}");
                Vec::new()
            }
        };
        raw.sort_by(|a, b| {
            a.output
                .cmp(&b.output)
                .then(a.idx.cmp(&b.idx))
                .then(a.id.cmp(&b.id))
        });

        // Whether to disambiguate labels by output. On the single-monitor
        // case — which is most of them — "Workspace 2" is what the user sees
        // in Niri's own overview and appending a connector name would be
        // noise.
        let multi_output = raw
            .iter()
            .filter_map(|w| w.output.as_deref())
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1;

        let table: Vec<WorkspaceRow> = raw
            .iter()
            .enumerate()
            .map(|(position, w)| WorkspaceRow {
                niri_id: w.id,
                index: position as i32,
                name: workspace_label(w, multi_output),
                output: w.output.clone(),
            })
            .collect();

        *self.workspaces.borrow_mut() = Some(table.clone());
        table
    }

    /// Translate a LoFi positional index back to the Niri workspace id.
    ///
    /// Reads the *cached* table rather than re-fetching, and that is the
    /// point. Niri destroys and recreates the trailing empty workspace on each
    /// output as focus moves away from it, so the id at a given position can
    /// change between LoFi's gather and the user's Enter. Re-fetching would
    /// resolve the position against whatever the table looks like *now* and
    /// could act on a workspace the user never saw; using the cached id means
    /// a workspace that has since been destroyed degrades to a no-op instead.
    /// Same trade the rest of the launcher makes — a silent no-op beats a
    /// surprising action (see `launch::activate`'s focus-vs-launch note).
    ///
    /// In practice the window is vanishingly small: LoFi's overlay holds
    /// keyboard focus for its whole lifetime, so the user cannot be moving
    /// around the compositor while it is open.
    fn niri_workspace_id(&self, index: i32) -> Option<u64> {
        self.workspace_table()
            .iter()
            .find(|row| row.index == index)
            .map(|row| row.niri_id)
    }

    /// The command target: the most-recently-focused window that isn't LoFi.
    ///
    /// The LoFi filter is belt-and-braces. Under Niri the launcher is a
    /// layer-shell surface, not a toplevel, so it never appears in `Windows`
    /// at all — but if layer-shell were unavailable and the launcher fell back
    /// to an ordinary window, without this filter every window command would
    /// resize or close the launcher itself.
    fn target_window<'a>(&self, windows: &'a [Window]) -> Option<&'a Window> {
        windows
            .iter()
            .find(|w| w.app_desktop_id.as_deref() != Some(LOFI_DESKTOP_ID))
    }

    /// Send an action, logging any failure. Every action method funnels
    /// through here so the `eprintln!`-and-degrade policy lives in one place.
    fn dispatch(&self, what: &str, action: Action) {
        if let Err(e) = ipc::action(action) {
            eprintln!("lofi: niri {what} failed: {e}");
        }
    }
}

impl Default for NiriBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Human-readable label for a Niri workspace.
///
/// Deliberately built from Niri's own `idx` (1-based, per output) rather than
/// from LoFi's positional index. The two agree on a single-monitor session,
/// and where they disagree the user's mental model comes from Niri's own
/// numbering and keybinds — so that is the number to show. LoFi's positional
/// index stays internal, used as a join key and never displayed.
///
/// A named workspace keeps its name verbatim, matching how the GNOME backend
/// passes through whatever label the shell reports.
fn workspace_label(workspace: &ipc::NiriWorkspace, multi_output: bool) -> String {
    if let Some(name) = workspace
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return name.to_string();
    }
    match (multi_output, workspace.output.as_deref()) {
        (true, Some(output)) => format!("Workspace {} ({output})", workspace.idx),
        _ => format!("Workspace {}", workspace.idx),
    }
}

impl Backend for NiriBackend {
    fn name(&self) -> &'static str {
        "niri"
    }

    /// Open windows, most-recently-focused first.
    ///
    /// Two things happen here that GNOME gets from its extension. First the
    /// MRU sort, because Niri returns windows in unspecified order. Then the
    /// `app_id` → desktop-entry resolution, which fills in the app name, icon,
    /// and the canonical `.desktop` id the app-to-window join in `main` needs.
    /// A window whose `app_id` matches nothing installed keeps its title and
    /// still focuses; it just carries no icon or app id.
    fn gather_windows(&self) -> Vec<Window> {
        let mut raw = match ipc::windows() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("lofi: niri list windows failed: {e}");
                return Vec::new();
            }
        };

        raw.sort_by_key(|w| std::cmp::Reverse(w.sort_key()));

        let workspace_index: HashMap<u64, i32> = self
            .workspace_table()
            .iter()
            .map(|row| (row.niri_id, row.index))
            .collect();

        raw.into_iter()
            .map(|w| {
                let resolved = w.app_id.as_deref().and_then(|id| self.apps.resolve(id));
                Window {
                    id: w.id,
                    title: w.title.unwrap_or_default(),
                    // Fall back to the raw app_id so a window whose app_id
                    // resolves to nothing still shows *something* useful in
                    // the row's app column.
                    app_name: resolved
                        .as_ref()
                        .and_then(|r| r.name.clone())
                        .or_else(|| w.app_id.clone()),
                    icon: resolved.as_ref().and_then(|r| r.icon.clone()),
                    workspace: w
                        .workspace_id
                        .and_then(|id| workspace_index.get(&id).copied())
                        .unwrap_or(UNKNOWN_WORKSPACE),
                    app_desktop_id: resolved.map(|r| r.desktop_id),
                }
            })
            .collect()
    }

    fn gather_workspaces(&self) -> Vec<Workspace> {
        self.workspace_table()
            .into_iter()
            .map(|row| Workspace {
                index: row.index,
                name: row.name,
            })
            .collect()
    }

    /// Window-action commands for the target window, or an empty `Vec` when
    /// no non-LoFi window is open — matching the GNOME backend, so the rows
    /// simply don't appear rather than appearing and doing nothing.
    ///
    /// `work_area` is the logical rectangle of the output the target window's
    /// workspace lives on, and `current_frame` is left zeroed. Neither is read
    /// by any command this backend emits: they exist on `Command` for the
    /// GNOME/macOS `compute_geometry` path, which has no Niri counterpart.
    /// `work_area` is filled in anyway because it is one cheap request and a
    /// real value is better than a lie; `current_frame` is not, because Niri
    /// reports a tiled window's on-screen position only while it is within the
    /// visible scroll region, so any value here would be right sometimes and
    /// silently wrong the rest of the time. A failed `Outputs` request
    /// degrades `work_area` to zeroes rather than dropping the whole command
    /// set — again unlike GNOME, where the geometry commands genuinely cannot
    /// work without it.
    fn gather_commands(&self, windows: &[Window]) -> Vec<Command> {
        let Some(target) = self.target_window(windows) else {
            return Vec::new();
        };

        let work_area = self.work_area_for(target).unwrap_or(WorkArea {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });

        ALL_KINDS
            .iter()
            .map(|&kind| Command {
                kind,
                target_window_id: target.id,
                work_area,
                current_frame: (0, 0, 0, 0),
            })
            .collect()
    }

    /// Workspace-move commands for the target window.
    ///
    /// The boundary logic (one absolute row per workspace, prev/next guarded
    /// at the ends, suppressed entirely for an unplaceable window) is
    /// `lofi_core::build_workspace_commands`, shared with GNOME. Only the
    /// absolute rows' labels are rewritten, so they read as the same workspace
    /// the switch rows name. Core builds them from the positional index, which
    /// is LoFi-internal and — on a multi-output session — is not the number
    /// Niri shows the user. See [`workspace_label`].
    fn gather_workspace_commands(
        &self,
        windows: &[Window],
        workspaces: &[Workspace],
    ) -> Vec<WorkspaceCommand> {
        let Some(target) = self.target_window(windows) else {
            return Vec::new();
        };

        let mut commands = build_workspace_commands(target.id, target.workspace, workspaces);
        for command in &mut commands {
            if command.kind == WorkspaceCommandKind::MoveToWorkspace
                && let Some(workspace) = workspaces.iter().find(|w| w.index == command.target_index)
            {
                command.name = format!("Move to {}", workspace.name);
            }
        }
        commands
    }

    fn gather_power_commands(&self) -> Vec<PowerCommand> {
        power::gather_power_commands()
    }

    fn focus_window(&self, id: u64) {
        self.dispatch("focus_window", Action::FocusWindow { id });
    }

    fn activate_workspace(&self, index: i32) {
        let Some(id) = self.niri_workspace_id(index) else {
            eprintln!("lofi: niri has no workspace at index {index}; ignoring");
            return;
        };
        self.dispatch(
            "focus_workspace",
            Action::FocusWorkspace {
                reference: WorkspaceReference::Id(id),
            },
        );
    }

    /// Dispatch a window-action command.
    ///
    /// The width presets go out as proportions of the working area, which is
    /// what Niri's `set-window-width "50%"` means. "Maximize column" is
    /// `SetWindowWidth` at 100% rather than Niri's `MaximizeColumn` action:
    /// Niri's own documentation defines maximize-column as equivalent to
    /// `set-column-width "100%"`, and `SetWindowWidth` takes a window id where
    /// `MaximizeColumn` would act on whatever is focused.
    ///
    /// The match is exhaustive rather than using a `_` arm, so adding a
    /// `CommandKind` variant forces a decision here instead of silently
    /// becoming a no-op.
    fn run_command(&self, command: &Command) {
        let id = command.target_window_id;
        match command.kind {
            CommandKind::Center => self.dispatch("center_window", Action::CenterWindow { id }),
            CommandKind::WidthThird => self.set_width(id, 100.0 / 3.0),
            CommandKind::WidthHalf => self.set_width(id, 50.0),
            CommandKind::WidthTwoThirds => self.set_width(id, 200.0 / 3.0),
            CommandKind::MaximizeColumn => self.set_width(id, 100.0),
            CommandKind::ExpandColumn => self.dispatch(
                "expand_column_to_available_width",
                Action::ExpandColumnToAvailableWidth {},
            ),
            CommandKind::ToggleFloating => self.dispatch(
                "toggle_window_floating",
                Action::ToggleWindowFloating { id },
            ),
            CommandKind::ToggleMaximize => self.dispatch(
                "maximize_window_to_edges",
                Action::MaximizeWindowToEdges { id },
            ),
            CommandKind::ToggleFullscreen => {
                self.dispatch("fullscreen_window", Action::FullscreenWindow { id })
            }
            CommandKind::Close => self.dispatch("close_window", Action::CloseWindow { id }),
            // The GNOME geometry kinds, `Minimize`, and the macOS-only
            // display moves. None appears in `ALL_KINDS`, so reaching one
            // would mean a gather/dispatch mismatch — log it rather than
            // failing silently.
            other => eprintln!(
                "niri: no dispatch for command kind {:?}; ignoring",
                other.as_id()
            ),
        }
    }

    /// Move the target window to `command.target_index` and follow it there.
    ///
    /// One call, not GNOME's two: Niri's `MoveWindowToWorkspace` takes a
    /// `focus` flag, so "move it and take me with it" is atomic. As on GNOME,
    /// the dispatch doesn't branch on `command.kind` — `target_index` is the
    /// already-resolved destination for the absolute and relative flavours
    /// alike.
    fn run_workspace_command(&self, command: &WorkspaceCommand) {
        let Some(workspace_id) = self.niri_workspace_id(command.target_index) else {
            eprintln!(
                "lofi: niri has no workspace at index {}; ignoring",
                command.target_index
            );
            return;
        };
        self.dispatch(
            "move_window_to_workspace",
            Action::MoveWindowToWorkspace {
                window_id: command.target_window_id,
                reference: WorkspaceReference::Id(workspace_id),
                focus: true,
            },
        );
    }

    fn run_power_command(&self, kind: PowerCommandKind) {
        power::activate(kind);
    }

    /// True: an ordinary toplevel would be tiled into the scrolling layout,
    /// pushing the user's windows aside every time the launcher opens. As a
    /// layer-shell overlay it floats above everything, takes keyboard focus
    /// without disturbing the layout, and — because a layer surface is not a
    /// toplevel — never shows up in its own window list.
    fn uses_layer_shell(&self) -> bool {
        true
    }
}

impl NiriBackend {
    /// `SetWindowWidth` as a percentage of the working area.
    fn set_width(&self, id: u64, percent: f64) {
        self.dispatch(
            "set_window_width",
            Action::SetWindowWidth {
                id,
                change: SizeChange::SetProportion(percent),
            },
        );
    }

    /// Logical rectangle of the output that owns `window`'s workspace.
    ///
    /// Two hops, because Niri reports geometry per output and membership per
    /// workspace: window → workspace → output connector name → logical rect.
    /// The first hop comes off the cached workspace table, so this costs one
    /// `Outputs` request and nothing else.
    ///
    /// Returns `None` if any hop fails or the output is disabled (a disabled
    /// output has no rectangle). Note this is the *whole* logical output, not
    /// the output minus layer-shell exclusive zones: Niri reports a bar's
    /// reserved space nowhere in its IPC. That is fine here only because no
    /// command this backend emits reads the value — see `gather_commands`.
    fn work_area_for(&self, window: &Window) -> Option<WorkArea> {
        let table = self.workspace_table();
        let output_name = table
            .iter()
            .find(|row| row.index == window.workspace)?
            .output
            .clone()?;

        let outputs = ipc::outputs()
            .map_err(|e| eprintln!("lofi: niri list outputs failed: {e}"))
            .ok()?;
        let logical = outputs.get(&output_name)?.logical?;

        Some(WorkArea {
            x: logical.x,
            y: logical.y,
            width: i32::try_from(logical.width).unwrap_or(0),
            height: i32::try_from(logical.height).unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(
        id: u64,
        idx: u32,
        name: Option<&str>,
        output: Option<&str>,
    ) -> ipc::NiriWorkspace {
        ipc::NiriWorkspace {
            id,
            idx,
            name: name.map(str::to_owned),
            output: output.map(str::to_owned),
        }
    }

    #[test]
    fn workspace_label_uses_niris_own_numbering() {
        assert_eq!(
            workspace_label(&workspace(9, 2, None, Some("DP-3")), false),
            "Workspace 2",
            "an unnamed workspace should be labelled with Niri's 1-based idx, \
             not with LoFi's positional index"
        );
    }

    #[test]
    fn workspace_label_prefers_a_custom_name() {
        assert_eq!(
            workspace_label(&workspace(9, 2, Some("editor"), Some("DP-3")), false),
            "editor",
            "a named workspace keeps its name verbatim"
        );
        assert_eq!(
            workspace_label(&workspace(9, 2, Some("editor"), Some("DP-3")), true),
            "editor",
            "a named workspace needs no output disambiguation"
        );
        // A whitespace-only name is not a name.
        assert_eq!(
            workspace_label(&workspace(9, 3, Some("   "), Some("DP-3")), false),
            "Workspace 3",
            "a blank name should fall back to the numbered label"
        );
    }

    #[test]
    fn workspace_label_disambiguates_by_output_only_when_needed() {
        assert_eq!(
            workspace_label(&workspace(9, 1, None, Some("HDMI-A-1")), true),
            "Workspace 1 (HDMI-A-1)",
            "with more than one output in play, the connector disambiguates \
             two workspaces that share an idx"
        );
        assert_eq!(
            workspace_label(&workspace(9, 1, None, Some("HDMI-A-1")), false),
            "Workspace 1",
            "on a single-output session the connector name would be noise"
        );
        assert_eq!(
            workspace_label(&workspace(9, 1, None, None), true),
            "Workspace 1",
            "a workspace with no output can't be disambiguated by one"
        );
    }

    #[test]
    fn all_kinds_are_dispatchable_and_exclude_geometry_and_minimize() {
        // Every emitted kind must have a real Niri action behind it. The
        // inverse — a kind we dispatch but never emit — is fine; the guard
        // that matters is not offering the user a row that does nothing.
        for kind in ALL_KINDS {
            assert!(
                !matches!(
                    kind,
                    CommandKind::Minimize
                        | CommandKind::NextDisplay
                        | CommandKind::PreviousDisplay
                        | CommandKind::CenterThird
                        | CommandKind::CenterHalf
                        | CommandKind::CenterTwoThirds
                        | CommandKind::LeftThird
                        | CommandKind::LeftHalf
                        | CommandKind::LeftTwoThirds
                        | CommandKind::RightThird
                        | CommandKind::RightHalf
                        | CommandKind::RightTwoThirds
                        | CommandKind::StandardSize
                ),
                "{kind:?} has no meaning under a scrolling tiler and must not be \
                 emitted by the Niri backend"
            );
        }

        // And the ids must be unique, or two rows would collide on one MRU key.
        let mut ids: Vec<&str> = ALL_KINDS.iter().map(|k| k.as_id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(
            count,
            ids.len(),
            "every emitted command kind must have a distinct id"
        );
    }
}
