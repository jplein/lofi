//! Pure-Rust geometry math for the window-action commands. No D-Bus, no GTK —
//! every input is captured by the platform layer (`lofi-linux`'s backends) at
//! gather time so this module stays trivially testable.

use crate::{CommandKind, WorkArea, Workspace, WorkspaceCommand, WorkspaceCommandKind};

/// Compute the target geometry for a window-action command, with no gaps.
///
/// Equivalent to [`compute_geometry_with_gaps`] with `gaps = 0`; kept as the
/// Linux crate's entry point so adding gaps for macOS did not have to touch a
/// crate that cannot be built on a Mac. GNOME could adopt gaps by calling the
/// gapped form directly.
pub fn compute_geometry(
    kind: CommandKind,
    work_area: &WorkArea,
    current_frame: (i32, i32, i32, i32),
) -> Option<(i32, i32, i32, i32)> {
    compute_geometry_with_gaps(kind, work_area, current_frame, 0)
}

/// Largest gap the arithmetic below will honour, in logical pixels. The gap
/// is doubled and added to screen coordinates, so it is clamped far below
/// `i32::MAX` to keep every intermediate in range; any gap this large has
/// already eaten the window and falls back to no gap (see below).
const MAX_GAP: u32 = i32::MAX as u32 / 4;

/// Compute the target geometry for a window-action command.
///
/// The work area is the monitor rectangle minus panel/dock struts; every
/// geometry command's output is anchored inside it. `current_frame` is
/// `(x, y, width, height)` of the target window's frame at gather time —
/// only `CommandKind::Center` reads it (it keeps the window's current size
/// and recenters), the other geometry commands ignore it.
///
/// `gaps` is the user's gap preference in logical pixels (`0` tiles edge to
/// edge and reproduces [`compute_geometry`] exactly). The rules, which exist
/// to make every gap on screen the *same* width whether it is between a
/// window and the work-area edge or between two windows:
///
/// - A window gets the full gap on every work-area edge it would otherwise
///   touch. Full-height commands get it top and bottom; left-anchored ones
///   get it on the left; right-anchored ones on the right.
/// - The seam between a left-anchored and a right-anchored window gets half
///   the gap from each side (`gaps / 2` from the left window, the remainder
///   from the right one, so an odd gap still sums exactly), which is what
///   makes `Left third` + `Right two-thirds` — or any complementary pair —
///   sit exactly one gap apart, the same distance as from the edges.
/// - `Center third` is inset by the same half gap on each side, because its
///   neighbours are `Left third` and `Right third` and the three should tile
///   with uniform gaps. `Center half` and `Center two-thirds` have no
///   neighbouring command that would fit beside them, so they keep their
///   full fraction of the width and get gaps top and bottom only.
/// - `Center` (recentre, keep size) and `Standard size` do not touch any
///   edge and ignore the gap altogether.
///
/// A gap so large that it would leave a window with no width or height
/// (`gaps = 600` on a 1000-pixel-tall work area) falls back to the ungapped
/// rectangle: a window that still appears beats one that vanishes, and a
/// preference that absurd is a typo, not an intent.
///
/// Returns `None` for every kind that isn't a rectangle: the state-toggle
/// commands (`Minimize`, `ToggleMaximize`, `ToggleFullscreen`), the
/// move-to-display commands, and the scrolling-tiler commands. The platform
/// layer dispatches all of those directly instead — see the match arms below
/// for why each one has no rectangle to compute. (The macOS maximize applies
/// the gap to all four edges itself, by insetting the work area it fills; see
/// `app/macos/Sources/LoFi/WindowCommands.swift`.)
pub fn compute_geometry_with_gaps(
    kind: CommandKind,
    work_area: &WorkArea,
    current_frame: (i32, i32, i32, i32),
    gaps: u32,
) -> Option<(i32, i32, i32, i32)> {
    // In range by construction: MAX_GAP is a quarter of i32::MAX.
    let gap = i32::try_from(gaps.min(MAX_GAP)).unwrap_or(i32::MAX / 4);
    let rect = gapped_geometry(kind, work_area, current_frame, gap)?;
    if gap > 0 && (rect.2 <= 0 || rect.3 <= 0) {
        // The gap ate the whole window: degrade to no gap rather than hand
        // the platform a rectangle it cannot show.
        return gapped_geometry(kind, work_area, current_frame, 0);
    }
    Some(rect)
}

/// The match behind [`compute_geometry_with_gaps`], with the gap already
/// clamped to a safe `i32`. Every arm is written so that `gap == 0` reproduces
/// the pre-gap formula byte for byte — the right-aligned anchoring and the
/// `(width - w) / 2` centring are unchanged, the gap terms are simply added
/// — so turning the feature on cannot move an ungapped window by a pixel.
fn gapped_geometry(
    kind: CommandKind,
    work_area: &WorkArea,
    current_frame: (i32, i32, i32, i32),
    gap: i32,
) -> Option<(i32, i32, i32, i32)> {
    // The seam between a left and a right window is split so the two halves
    // always sum to exactly one gap, odd values included: the left window
    // gives up `near`, the right window `far`.
    let near = gap / 2;
    let far = gap - near;
    // Every full-height command shares the same vertical inset.
    let top = work_area.y + gap;
    let full_height = work_area.height - 2 * gap;

    match kind {
        CommandKind::Center => {
            let (_, _, w, h) = current_frame;
            Some((
                work_area.x + (work_area.width - w) / 2,
                work_area.y + (work_area.height - h) / 2,
                w,
                h,
            ))
        }
        // Inset by half a gap on each side so it tiles with Left third and
        // Right third, which give up the other half of each seam.
        CommandKind::CenterThird => {
            let w = work_area.width / 3 - gap;
            Some((work_area.x + (work_area.width - w) / 2, top, w, full_height))
        }
        // Center half and Center two-thirds touch no vertical edge and have
        // no neighbouring command that fits beside them, so only the
        // vertical gap applies.
        CommandKind::CenterHalf => {
            let w = work_area.width / 2;
            Some((work_area.x + work_area.width / 4, top, w, full_height))
        }
        CommandKind::CenterTwoThirds => {
            let w = work_area.width * 2 / 3;
            Some((work_area.x + (work_area.width - w) / 2, top, w, full_height))
        }
        // Left-anchored: full gap on the left edge, `near` at the seam.
        CommandKind::LeftThird => Some((
            work_area.x + gap,
            top,
            work_area.width / 3 - gap - near,
            full_height,
        )),
        CommandKind::LeftHalf => Some((
            work_area.x + gap,
            top,
            work_area.width / 2 - gap - near,
            full_height,
        )),
        CommandKind::LeftTwoThirds => Some((
            work_area.x + gap,
            top,
            work_area.width * 2 / 3 - gap - near,
            full_height,
        )),
        // Right-aligned variants anchor by `x + width - w` (not `x + w`) so
        // the window stays flush against the work area's right edge even when
        // integer division leaves `w` a pixel short of an exact fraction.
        // With a gap: `far` at the seam, the full gap on the right edge.
        CommandKind::RightThird => {
            let w = work_area.width / 3;
            Some((
                work_area.x + work_area.width - w + far,
                top,
                w - far - gap,
                full_height,
            ))
        }
        CommandKind::RightHalf => {
            let w = work_area.width / 2;
            Some((work_area.x + w + far, top, w - far - gap, full_height))
        }
        CommandKind::RightTwoThirds => {
            let w = work_area.width * 2 / 3;
            Some((
                work_area.x + work_area.width - w + far,
                top,
                w - far - gap,
                full_height,
            ))
        }
        CommandKind::StandardSize => {
            let w = work_area.width * 2 / 3;
            let h = work_area.height * 2 / 3;
            Some((
                work_area.x + (work_area.width - w) / 2,
                work_area.y + (work_area.height - h) / 2,
                w,
                h,
            ))
        }
        CommandKind::Minimize | CommandKind::ToggleMaximize | CommandKind::ToggleFullscreen => None,
        // Move-to-display commands depend on multi-display geometry the
        // platform layer holds (set of displays + the target's current
        // one), not on a single work area. Returning None routes them
        // through the platform's state-toggle dispatch path, which
        // computes the destination rect at activation time.
        CommandKind::NextDisplay | CommandKind::PreviousDisplay => None,
        // Scrolling-tiler commands. These don't produce a rectangle at all:
        // the width presets are a *proportion* the compositor resolves
        // against its own working area (Niri's `SetWindowWidth` takes a
        // percentage), and the rest are state changes. Computing a rectangle
        // here and sending it would also be wrong in a subtler way — under a
        // scrolling tiler a window's x/y is a consequence of its place in the
        // scroll order, so any position this function invented would be
        // immediately overruled by the layout.
        CommandKind::WidthThird
        | CommandKind::WidthHalf
        | CommandKind::WidthTwoThirds
        | CommandKind::MaximizeColumn
        | CommandKind::ExpandColumn
        | CommandKind::ToggleFloating
        | CommandKind::Close => None,
    }
}

/// Build the dynamic set of workspace-move commands for the target window.
///
/// `target_window_id` is the window every emitted command acts on (the
/// previously-focused user window the platform layer captured at gather time,
/// the same target as `compute_geometry`'s commands). `target_workspace` is
/// that window's current 0-based Mutter workspace index — or a negative value
/// for a sticky / on-all-workspaces window, in which case the relative
/// prev/next moves are omitted (there's no single "current" workspace to step
/// from). `workspaces` is the full set of open workspaces in index order, as
/// returned by the platform's workspace gatherer.
///
/// The result, in order, is:
///
/// 1. One `MoveToWorkspace` per open workspace ("Move to workspace N", 1-based
///    label), including the window's current workspace — moving a window to the
///    workspace it's already on is a harmless no-op, and a complete, stable
///    list keeps each destination's MRU rank and the user's muscle memory
///    consistent across launches.
/// 2. `MoveToPreviousWorkspace` ("Move to previous workspace"), unless the
///    window is already on the first workspace (index 0) or is sticky.
/// 3. `MoveToNextWorkspace` ("Move to next workspace"), unless the window is
///    already on the last workspace (index `len - 1`) or is sticky.
///
/// Pure and platform-free for the same reason as `compute_geometry`: the
/// platform layer supplies the gathered inputs and this function does the
/// labelling / boundary logic, so it stays trivially unit-testable.
pub fn build_workspace_commands(
    target_window_id: u64,
    target_workspace: i32,
    workspaces: &[Workspace],
) -> Vec<WorkspaceCommand> {
    let mut out = Vec::with_capacity(workspaces.len() + 2);

    // Absolute: one row per open workspace, in index order. The label is
    // 1-based to match how GNOME numbers workspaces in its own UI.
    for ws in workspaces {
        out.push(WorkspaceCommand {
            kind: WorkspaceCommandKind::MoveToWorkspace,
            target_window_id,
            target_index: ws.index,
            name: format!("Move to workspace {}", ws.index + 1),
        });
    }

    let last_index = workspaces.len() as i32 - 1;

    // Relative previous: only when the window sits on a real workspace that
    // has one below it. `target_workspace > 0` excludes both the first
    // workspace and any sticky/negative index; `<= last_index` guards against
    // an out-of-range current index.
    if target_workspace > 0 && target_workspace <= last_index {
        out.push(WorkspaceCommand {
            kind: WorkspaceCommandKind::MoveToPreviousWorkspace,
            target_window_id,
            target_index: target_workspace - 1,
            name: "Move to previous workspace".to_string(),
        });
    }

    // Relative next: only when the window sits on a real workspace that has
    // one above it. `target_workspace >= 0` excludes sticky/negative indices;
    // `< last_index` excludes the last workspace.
    if target_workspace >= 0 && target_workspace < last_index {
        out.push(WorkspaceCommand {
            kind: WorkspaceCommandKind::MoveToNextWorkspace,
            target_window_id,
            target_index: target_workspace + 1,
            name: "Move to next workspace".to_string(),
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use crate::{
        CommandKind, WorkArea, Workspace, WorkspaceCommandKind, commands::build_workspace_commands,
        commands::compute_geometry, commands::compute_geometry_with_gaps,
    };

    /// Fixed work area used by every test in this module. Non-zero `x`/`y`
    /// catches relative-vs-absolute-position bugs; the dimensions are picked so
    /// integer divisions land on whole numbers for the half / two-thirds cases.
    const WA: WorkArea = WorkArea {
        x: 100,
        y: 50,
        width: 1800,
        height: 1000,
    };

    /// Current frame used by the Center test. Center recenters without
    /// resizing, so this frame's `width`/`height` (800x600) must appear
    /// unchanged in the computed geometry.
    const FRAME: (i32, i32, i32, i32) = (200, 60, 800, 600);

    /// Placeholder frame for tests whose geometry doesn't read the frame.
    const ZERO_FRAME: (i32, i32, i32, i32) = (0, 0, 0, 0);

    #[test]
    fn center_geometry_uses_current_frame_size_and_centers_in_work_area() {
        // Center keeps the window's current size and centers it within the
        // work area. With WA={x:100,y:50,w:1800,h:1000} and frame size 800x600,
        // x = 100 + (1800-800)/2 = 600, y = 50 + (1000-600)/2 = 250.
        let actual = compute_geometry(CommandKind::Center, &WA, FRAME);
        let expected = Some((600, 250, 800, 600));
        assert_eq!(
            actual, expected,
            "Center should center current frame in work area; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn center_third_geometry() {
        // Center third: width/3 x full height, centered horizontally.
        // w = 1800/3 = 600, x = 100 + (1800-600)/2 = 700, y = 50, h = 1000.
        let actual = compute_geometry(CommandKind::CenterThird, &WA, ZERO_FRAME);
        let expected = Some((700, 50, 600, 1000));
        assert_eq!(
            actual, expected,
            "CenterThird should be width/3 x full height centered; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn center_half_geometry() {
        // Center half: width/2 x full height, centered horizontally.
        // x = 100 + 1800/4 = 550, y = 50, w = 900, h = 1000.
        let actual = compute_geometry(CommandKind::CenterHalf, &WA, ZERO_FRAME);
        let expected = Some((550, 50, 900, 1000));
        assert_eq!(
            actual, expected,
            "CenterHalf should be width/2 x full height centered; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn center_two_thirds_geometry() {
        // Center two-thirds: width*2/3 x full height, centered horizontally.
        // w = 1800*2/3 = 1200, x = 100 + (1800-1200)/2 = 400, y = 50, h = 1000.
        let actual = compute_geometry(CommandKind::CenterTwoThirds, &WA, ZERO_FRAME);
        let expected = Some((400, 50, 1200, 1000));
        assert_eq!(
            actual, expected,
            "CenterTwoThirds should be width*2/3 x full height centered; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn left_half_geometry() {
        // Left half: width/2 x full height, flush left.
        let actual = compute_geometry(CommandKind::LeftHalf, &WA, ZERO_FRAME);
        let expected = Some((100, 50, 900, 1000));
        assert_eq!(
            actual, expected,
            "LeftHalf should be width/2 x full height flush left; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn right_half_geometry() {
        // Right half: width/2 x full height, flush right.
        // x = 100 + 900 = 1000, w = 900.
        let actual = compute_geometry(CommandKind::RightHalf, &WA, ZERO_FRAME);
        let expected = Some((1000, 50, 900, 1000));
        assert_eq!(
            actual, expected,
            "RightHalf should be width/2 x full height flush right; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn left_third_geometry() {
        // Left third: width/3 x full height, flush left.
        // w = 1800/3 = 600.
        let actual = compute_geometry(CommandKind::LeftThird, &WA, ZERO_FRAME);
        let expected = Some((100, 50, 600, 1000));
        assert_eq!(
            actual, expected,
            "LeftThird should be width/3 x full height flush left; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn left_two_thirds_geometry() {
        // Left two-thirds: width*2/3 x full height, flush left.
        // w = 1800*2/3 = 1200.
        let actual = compute_geometry(CommandKind::LeftTwoThirds, &WA, ZERO_FRAME);
        let expected = Some((100, 50, 1200, 1000));
        assert_eq!(
            actual, expected,
            "LeftTwoThirds should be width*2/3 x full height flush left; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn right_third_geometry() {
        // Right third: width/3 x full height, flush right.
        // w = 1800/3 = 600, x = 100 + 1800 - 600 = 1300.
        let actual = compute_geometry(CommandKind::RightThird, &WA, ZERO_FRAME);
        let expected = Some((1300, 50, 600, 1000));
        assert_eq!(
            actual, expected,
            "RightThird should be width/3 x full height flush right; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn right_two_thirds_geometry() {
        // Right two-thirds: width*2/3 x full height, flush right.
        // w = 1800*2/3 = 1200, x = 100 + 1800 - 1200 = 700.
        let actual = compute_geometry(CommandKind::RightTwoThirds, &WA, ZERO_FRAME);
        let expected = Some((700, 50, 1200, 1000));
        assert_eq!(
            actual, expected,
            "RightTwoThirds should be width*2/3 x full height flush right; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn standard_size_geometry() {
        // Standard size: width*2/3 x height*2/3, centered.
        // w = 1800*2/3 = 1200, h = 1000*2/3 = 666 (integer division).
        // x = 100 + (1800-1200)/2 = 400, y = 50 + (1000-666)/2 = 217.
        let actual = compute_geometry(CommandKind::StandardSize, &WA, ZERO_FRAME);
        let expected = Some((400, 217, 1200, 666));
        assert_eq!(
            actual, expected,
            "StandardSize should be 2/3 x 2/3 centered; got {actual:?}, want {expected:?}"
        );
    }

    #[test]
    fn minimize_returns_none() {
        // Minimize is a state-toggle, not a geometry change.
        let actual = compute_geometry(CommandKind::Minimize, &WA, ZERO_FRAME);
        assert_eq!(
            actual, None,
            "Minimize is a state command and must return None; got {actual:?}"
        );
    }

    #[test]
    fn toggle_maximize_returns_none() {
        let actual = compute_geometry(CommandKind::ToggleMaximize, &WA, ZERO_FRAME);
        assert_eq!(
            actual, None,
            "ToggleMaximize is a state command and must return None; got {actual:?}"
        );
    }

    #[test]
    fn toggle_fullscreen_returns_none() {
        let actual = compute_geometry(CommandKind::ToggleFullscreen, &WA, ZERO_FRAME);
        assert_eq!(
            actual, None,
            "ToggleFullscreen is a state command and must return None; got {actual:?}"
        );
    }

    #[test]
    fn next_display_returns_none() {
        // NextDisplay depends on multi-display geometry the platform layer
        // holds (set of displays + the target's current one), not the
        // single work area threaded through this function. The platform
        // dispatch computes the destination rect at activation time.
        let actual = compute_geometry(CommandKind::NextDisplay, &WA, ZERO_FRAME);
        assert_eq!(
            actual, None,
            "NextDisplay is platform-dispatched and must return None; got {actual:?}"
        );
    }

    #[test]
    fn previous_display_returns_none() {
        let actual = compute_geometry(CommandKind::PreviousDisplay, &WA, ZERO_FRAME);
        assert_eq!(
            actual, None,
            "PreviousDisplay is platform-dispatched and must return None; got {actual:?}"
        );
    }

    #[test]
    fn scrolling_tiler_kinds_return_none() {
        // The Niri command kinds are proportions and state changes, not
        // rectangles. Returning a rectangle for any of them would be actively
        // harmful: under a scrolling tiler the position component would be
        // overruled by the layout, so the platform layer would be sending
        // geometry that silently doesn't apply.
        for kind in [
            CommandKind::WidthThird,
            CommandKind::WidthHalf,
            CommandKind::WidthTwoThirds,
            CommandKind::MaximizeColumn,
            CommandKind::ExpandColumn,
            CommandKind::ToggleFloating,
            CommandKind::Close,
        ] {
            // Pass a non-zero frame too, so a kind that accidentally fell
            // into the Center arm would surface as Some(...) rather than
            // coincidentally matching a zeroed expectation.
            assert_eq!(
                compute_geometry(kind, &WA, FRAME),
                None,
                "{kind:?} is platform-dispatched under a scrolling tiler and must return None"
            );
        }
    }

    /// Every geometry kind, for the gap tests that sweep the whole set.
    const GEOMETRY_KINDS: [CommandKind; 11] = [
        CommandKind::Center,
        CommandKind::CenterThird,
        CommandKind::CenterHalf,
        CommandKind::CenterTwoThirds,
        CommandKind::LeftThird,
        CommandKind::LeftHalf,
        CommandKind::LeftTwoThirds,
        CommandKind::RightThird,
        CommandKind::RightHalf,
        CommandKind::RightTwoThirds,
        CommandKind::StandardSize,
    ];

    /// Right edge (exclusive) of a `(x, y, w, h)` rectangle.
    fn right_edge(rect: (i32, i32, i32, i32)) -> i32 {
        rect.0 + rect.2
    }

    #[test]
    fn zero_gap_is_byte_identical_to_the_ungapped_geometry() {
        // The gap terms are additive on top of the original formulas, so a
        // default configuration must not move any window by a pixel. Swept
        // over an awkward work area too, where integer division rounds.
        let odd = WorkArea {
            x: 7,
            y: 3,
            width: 1801,
            height: 1003,
        };
        for wa in [WA, odd] {
            for kind in GEOMETRY_KINDS {
                assert_eq!(
                    compute_geometry_with_gaps(kind, &wa, FRAME, 0),
                    compute_geometry(kind, &wa, FRAME),
                    "{kind:?} with gaps = 0 must equal the ungapped geometry"
                );
            }
        }
    }

    #[test]
    fn left_half_gap_is_full_on_three_edges_and_half_at_the_seam() {
        // x = 100 + 8, y = 50 + 8, w = 900 - 8 - 4, h = 1000 - 16.
        assert_eq!(
            compute_geometry_with_gaps(CommandKind::LeftHalf, &WA, ZERO_FRAME, 8),
            Some((108, 58, 888, 984))
        );
    }

    #[test]
    fn right_half_gap_is_full_on_three_edges_and_half_at_the_seam() {
        // x = 100 + 900 + 4, w = 900 - 4 - 8; the right edge lands at
        // 1004 + 888 = 1892 = 1900 - 8, one gap in from the work area.
        let rect = compute_geometry_with_gaps(CommandKind::RightHalf, &WA, ZERO_FRAME, 8);
        assert_eq!(rect, Some((1004, 58, 888, 984)));
        assert_eq!(right_edge(rect.unwrap()), WA.x + WA.width - 8);
    }

    #[test]
    fn complementary_left_and_right_windows_sit_exactly_one_gap_apart() {
        // The whole point of the half-gap seam: any left fraction next to the
        // right fraction that completes it leaves exactly `gaps` between the
        // two windows, the same distance each keeps from the edges.
        for (left, right) in [
            (CommandKind::LeftHalf, CommandKind::RightHalf),
            (CommandKind::LeftThird, CommandKind::RightTwoThirds),
            (CommandKind::LeftTwoThirds, CommandKind::RightThird),
        ] {
            for gaps in [1u32, 2, 7, 8, 9, 24] {
                let l = compute_geometry_with_gaps(left, &WA, ZERO_FRAME, gaps).unwrap();
                let r = compute_geometry_with_gaps(right, &WA, ZERO_FRAME, gaps).unwrap();
                assert_eq!(
                    r.0 - right_edge(l),
                    gaps as i32,
                    "{left:?} + {right:?} with gaps = {gaps} should leave one gap between them"
                );
                assert_eq!(
                    l.0,
                    WA.x + gaps as i32,
                    "{left:?} keeps a full gap on the left"
                );
                assert_eq!(
                    right_edge(r),
                    WA.x + WA.width - gaps as i32,
                    "{right:?} keeps a full gap on the right"
                );
                for rect in [l, r] {
                    assert_eq!(rect.1, WA.y + gaps as i32, "full gap on top");
                    assert_eq!(
                        rect.1 + rect.3,
                        WA.y + WA.height - gaps as i32,
                        "full gap on the bottom"
                    );
                }
            }
        }
    }

    #[test]
    fn an_odd_gap_still_sums_exactly_at_the_seam() {
        // 9 splits as 4 (left window) + 5 (right window). The left half ends
        // at 100 + 9 + 887 = 996; the right half starts at 100 + 900 + 5 =
        // 1005; 1005 - 996 = 9.
        assert_eq!(
            compute_geometry_with_gaps(CommandKind::LeftHalf, &WA, ZERO_FRAME, 9),
            Some((109, 59, 887, 982))
        );
        assert_eq!(
            compute_geometry_with_gaps(CommandKind::RightHalf, &WA, ZERO_FRAME, 9),
            Some((1005, 59, 886, 982))
        );
    }

    #[test]
    fn thirds_tile_with_uniform_gaps() {
        // Left third | Center third | Right third, gaps = 8: both seams are
        // exactly 8 wide, so the center third gives up half a gap each side.
        let l = compute_geometry_with_gaps(CommandKind::LeftThird, &WA, ZERO_FRAME, 8).unwrap();
        let c = compute_geometry_with_gaps(CommandKind::CenterThird, &WA, ZERO_FRAME, 8).unwrap();
        let r = compute_geometry_with_gaps(CommandKind::RightThird, &WA, ZERO_FRAME, 8).unwrap();
        assert_eq!(l, (108, 58, 588, 984));
        assert_eq!(c, (704, 58, 592, 984));
        assert_eq!(r, (1304, 58, 588, 984));
        assert_eq!(c.0 - right_edge(l), 8, "left | center seam");
        assert_eq!(r.0 - right_edge(c), 8, "center | right seam");
    }

    #[test]
    fn center_half_and_two_thirds_get_vertical_gaps_only() {
        // Neither touches a vertical edge and neither has a neighbouring
        // command that would fit beside it, so the width is the plain
        // fraction and only y / height move.
        assert_eq!(
            compute_geometry_with_gaps(CommandKind::CenterHalf, &WA, ZERO_FRAME, 8),
            Some((550, 58, 900, 984))
        );
        assert_eq!(
            compute_geometry_with_gaps(CommandKind::CenterTwoThirds, &WA, ZERO_FRAME, 8),
            Some((400, 58, 1200, 984))
        );
    }

    #[test]
    fn center_and_standard_size_ignore_the_gap() {
        // Neither touches an edge: Center keeps the window's own size and
        // Standard size is two-thirds floating in the middle.
        for kind in [CommandKind::Center, CommandKind::StandardSize] {
            assert_eq!(
                compute_geometry_with_gaps(kind, &WA, FRAME, 8),
                compute_geometry(kind, &WA, FRAME),
                "{kind:?} should not change with a gap"
            );
        }
    }

    #[test]
    fn a_gap_that_would_eat_the_window_falls_back_to_no_gap() {
        // 600 on a 1000-tall work area leaves a negative height; a window
        // that still appears beats one that vanishes.
        assert_eq!(
            compute_geometry_with_gaps(CommandKind::LeftHalf, &WA, ZERO_FRAME, 600),
            compute_geometry(CommandKind::LeftHalf, &WA, ZERO_FRAME)
        );
        // And the extremes must not overflow or panic.
        for kind in GEOMETRY_KINDS {
            assert_eq!(
                compute_geometry_with_gaps(kind, &WA, FRAME, u32::MAX),
                compute_geometry(kind, &WA, FRAME),
                "{kind:?} with an absurd gap should degrade to the ungapped geometry"
            );
        }
    }

    #[test]
    fn non_geometry_kinds_stay_none_with_a_gap() {
        for kind in [
            CommandKind::Minimize,
            CommandKind::ToggleMaximize,
            CommandKind::ToggleFullscreen,
            CommandKind::NextDisplay,
            CommandKind::PreviousDisplay,
            CommandKind::WidthHalf,
            CommandKind::Close,
        ] {
            assert_eq!(compute_geometry_with_gaps(kind, &WA, FRAME, 8), None);
        }
    }

    /// Window id every `build_workspace_commands` test threads through; non-zero
    /// so a dropped/zeroed target id would surface as a mismatch.
    const TARGET_ID: u64 = 77;

    /// Build a workspace fixture with `count` workspaces at contiguous 0-based
    /// indices, named the way the GNOME extension names them ("Workspace N",
    /// 1-based). Only `index` is read by `build_workspace_commands`, but the
    /// name is filled to match real gather output.
    fn workspaces(count: i32) -> Vec<Workspace> {
        (0..count)
            .map(|i| Workspace {
                index: i,
                name: format!("Workspace {}", i + 1),
            })
            .collect()
    }

    /// Collect `(kind, target_index, name, as_id)` tuples so tests can assert on
    /// the whole emitted set in one comparison.
    fn rows(cmds: &[crate::WorkspaceCommand]) -> Vec<(WorkspaceCommandKind, i32, String, String)> {
        cmds.iter()
            .map(|c| (c.kind, c.target_index, c.name.clone(), c.as_id()))
            .collect()
    }

    #[test]
    fn workspace_commands_emit_one_absolute_per_workspace_plus_prev_and_next() {
        // Window on workspace index 1 (the 2nd of 4): every absolute move is
        // present (including a no-op move to its own workspace 2), plus both
        // prev (→0) and next (→2) because it's neither first nor last.
        let cmds = build_workspace_commands(TARGET_ID, 1, &workspaces(4));
        assert_eq!(
            rows(&cmds),
            vec![
                (
                    WorkspaceCommandKind::MoveToWorkspace,
                    0,
                    "Move to workspace 1".into(),
                    "move_to_workspace_0".into()
                ),
                (
                    WorkspaceCommandKind::MoveToWorkspace,
                    1,
                    "Move to workspace 2".into(),
                    "move_to_workspace_1".into()
                ),
                (
                    WorkspaceCommandKind::MoveToWorkspace,
                    2,
                    "Move to workspace 3".into(),
                    "move_to_workspace_2".into()
                ),
                (
                    WorkspaceCommandKind::MoveToWorkspace,
                    3,
                    "Move to workspace 4".into(),
                    "move_to_workspace_3".into()
                ),
                (
                    WorkspaceCommandKind::MoveToPreviousWorkspace,
                    0,
                    "Move to previous workspace".into(),
                    "move_to_previous_workspace".into()
                ),
                (
                    WorkspaceCommandKind::MoveToNextWorkspace,
                    2,
                    "Move to next workspace".into(),
                    "move_to_next_workspace".into()
                ),
            ],
            "expected 4 absolute moves (including the current workspace) plus prev→0 and next→2"
        );
        assert!(
            cmds.iter().all(|c| c.target_window_id == TARGET_ID),
            "every command must carry the target window id"
        );
    }

    #[test]
    fn workspace_commands_omit_previous_on_first_workspace() {
        // On the first workspace (index 0): no "previous", but "next" → 1.
        let cmds = build_workspace_commands(TARGET_ID, 0, &workspaces(3));
        let kinds: Vec<WorkspaceCommandKind> = cmds.iter().map(|c| c.kind).collect();
        assert!(
            !kinds.contains(&WorkspaceCommandKind::MoveToPreviousWorkspace),
            "no 'previous' command when already on the first workspace; got {kinds:?}"
        );
        let next = cmds
            .iter()
            .find(|c| c.kind == WorkspaceCommandKind::MoveToNextWorkspace)
            .expect("a 'next' command should be present on the first of three workspaces");
        assert_eq!(
            next.target_index, 1,
            "'next' from workspace 0 should target index 1"
        );
    }

    #[test]
    fn workspace_commands_omit_next_on_last_workspace() {
        // On the last workspace (index 2 of 3): no "next", but "previous" → 1.
        let cmds = build_workspace_commands(TARGET_ID, 2, &workspaces(3));
        let kinds: Vec<WorkspaceCommandKind> = cmds.iter().map(|c| c.kind).collect();
        assert!(
            !kinds.contains(&WorkspaceCommandKind::MoveToNextWorkspace),
            "no 'next' command when already on the last workspace; got {kinds:?}"
        );
        let prev = cmds
            .iter()
            .find(|c| c.kind == WorkspaceCommandKind::MoveToPreviousWorkspace)
            .expect("a 'previous' command should be present on the last of three workspaces");
        assert_eq!(
            prev.target_index, 1,
            "'previous' from workspace 2 should target index 1"
        );
    }

    #[test]
    fn workspace_commands_single_workspace_has_neither_prev_nor_next() {
        // One workspace: the window is simultaneously on the first and last, so
        // both relative moves are omitted; only the single absolute remains.
        let cmds = build_workspace_commands(TARGET_ID, 0, &workspaces(1));
        assert_eq!(
            rows(&cmds),
            vec![(
                WorkspaceCommandKind::MoveToWorkspace,
                0,
                "Move to workspace 1".into(),
                "move_to_workspace_0".into()
            )],
            "a single-workspace session should emit only the one absolute move"
        );
    }

    #[test]
    fn workspace_commands_sticky_window_omits_relative_moves() {
        // A sticky / on-all-workspaces window reports workspace -1. There's no
        // single "current" workspace to step from, so prev/next are omitted —
        // but the absolute moves (which un-stick and place the window) remain.
        let cmds = build_workspace_commands(TARGET_ID, -1, &workspaces(3));
        let kinds: Vec<WorkspaceCommandKind> = cmds.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![WorkspaceCommandKind::MoveToWorkspace; 3],
            "a sticky window should get only the three absolute moves; got {kinds:?}"
        );
    }

    #[test]
    fn workspace_commands_empty_workspace_set_is_empty() {
        // Degenerate input (the gatherer returned nothing): no rows at all,
        // and no panic from the `len - 1` boundary arithmetic.
        let cmds = build_workspace_commands(TARGET_ID, 0, &workspaces(0));
        assert!(
            cmds.is_empty(),
            "no workspaces means no workspace-move commands; got {} rows",
            cmds.len()
        );
    }
}
