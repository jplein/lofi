//! Pure-Rust geometry math for the window-action commands. No D-Bus, no GTK —
//! every input is captured by the platform layer (`lofi-gnome::commands`) at
//! gather time so this module stays trivially testable.

use crate::{CommandKind, WorkArea};

/// Compute the target geometry for a window-action command.
///
/// The work area is the monitor rectangle minus panel/dock struts; every
/// geometry command's output is anchored inside it. `current_frame` is
/// `(x, y, width, height)` of the target window's frame at gather time —
/// only `CommandKind::Center` reads it (it keeps the window's current size
/// and recenters), the other geometry commands ignore it.
///
/// Returns `None` for the state-toggle commands (`Minimize`,
/// `ToggleMaximize`, `ToggleFullscreen`) which don't produce a rectangle;
/// the activation path dispatches those to dedicated D-Bus methods instead.
pub fn compute_geometry(
    kind: CommandKind,
    work_area: &WorkArea,
    current_frame: (i32, i32, i32, i32),
) -> Option<(i32, i32, i32, i32)> {
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
        CommandKind::CenterThird => {
            let w = work_area.width / 3;
            Some((
                work_area.x + (work_area.width - w) / 2,
                work_area.y,
                w,
                work_area.height,
            ))
        }
        CommandKind::CenterHalf => {
            let w = work_area.width / 2;
            Some((
                work_area.x + work_area.width / 4,
                work_area.y,
                w,
                work_area.height,
            ))
        }
        CommandKind::CenterTwoThirds => {
            let w = work_area.width * 2 / 3;
            Some((
                work_area.x + (work_area.width - w) / 2,
                work_area.y,
                w,
                work_area.height,
            ))
        }
        CommandKind::LeftThird => Some((
            work_area.x,
            work_area.y,
            work_area.width / 3,
            work_area.height,
        )),
        CommandKind::LeftHalf => Some((
            work_area.x,
            work_area.y,
            work_area.width / 2,
            work_area.height,
        )),
        CommandKind::LeftTwoThirds => Some((
            work_area.x,
            work_area.y,
            work_area.width * 2 / 3,
            work_area.height,
        )),
        // Right-aligned variants anchor by `x + width - w` (not `x + w`) so
        // the window stays flush against the work area's right edge even when
        // integer division leaves `w` a pixel short of an exact fraction.
        CommandKind::RightThird => {
            let w = work_area.width / 3;
            Some((work_area.x + work_area.width - w, work_area.y, w, work_area.height))
        }
        CommandKind::RightHalf => {
            let w = work_area.width / 2;
            Some((work_area.x + w, work_area.y, w, work_area.height))
        }
        CommandKind::RightTwoThirds => {
            let w = work_area.width * 2 / 3;
            Some((work_area.x + work_area.width - w, work_area.y, w, work_area.height))
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
    }
}

#[cfg(test)]
mod tests {
    use crate::{CommandKind, WorkArea, commands::compute_geometry};

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
}
