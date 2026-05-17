//! Gather the launcher's static window-action command set, populated with
//! the captured target window plus its monitor's work area and current
//! frame. The target is the most-recently-focused user window (excluding
//! LoFi itself, which is the focused window while the launcher is open —
//! see decision 8 in the plan).

use crate::windows;
use lofi_core::{Command, CommandKind};

/// Canonical desktop id for the launcher itself. We compare against this to
/// skip LoFi when picking the target window — otherwise every command would
/// resize/minimize the launcher window itself.
const LOFI_DESKTOP_ID: &str = "dev.jplein.LoFi.desktop";

/// Every command kind, in the order they appear in the launcher list.
const ALL_KINDS: &[CommandKind] = &[
    CommandKind::Center,
    CommandKind::CenterHalf,
    CommandKind::CenterTwoThirds,
    CommandKind::LeftHalf,
    CommandKind::RightHalf,
    CommandKind::StandardSize,
    CommandKind::Minimize,
    CommandKind::ToggleMaximize,
    CommandKind::ToggleFullscreen,
];

/// Gather the static command set, populated with the captured target window
/// and its monitor's work area. Returns an empty Vec when there's no usable
/// target (no non-LoFi windows open) or the work-area / frame D-Bus reads
/// fail — matches the original window-commands set's `if (!window) return
/// false` guard. The empty result drops the command rows from the launcher
/// list entirely; users who launch LoFi with no other windows open just
/// don't see them.
pub fn gather_commands() -> Vec<Command> {
    let windows_vec = windows::gather_windows();
    let target = windows_vec
        .into_iter()
        .find(|w| w.app_desktop_id.as_deref() != Some(LOFI_DESKTOP_ID));
    let Some(target) = target else {
        return Vec::new();
    };
    let Some(work_area) = windows::get_window_work_area(target.id) else {
        return Vec::new();
    };
    let Some(current_frame) = windows::get_window_frame(target.id) else {
        return Vec::new();
    };

    ALL_KINDS
        .iter()
        .map(|&kind| Command {
            kind,
            target_window_id: target.id,
            work_area,
            current_frame,
        })
        .collect()
}
