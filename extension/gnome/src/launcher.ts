import type Meta from 'gi://Meta';
import Shell from 'gi://Shell';

/// The launcher's GApplication id, set in `app/gnome/src/main.rs` via
/// `adw::Application::builder().application_id(APP_ID)`. On Wayland this
/// reaches the compositor as the xdg-shell `app_id`; on X11 GTK derives
/// `WM_CLASS` from it. Identifying by this id rather than by a Shell.App
/// id means the match works even when no `.desktop` file is installed
/// (the common case for ad-hoc invocations / dev builds).
const LOFI_GAPP_ID = 'dev.jplein.LoFi';

/// Suffixed form used when a Shell.App *is* resolved (i.e. when a
/// `dev.jplein.LoFi.desktop` file is installed under XDG_DATA_DIRS).
/// Matches `LOFI_DESKTOP_ID` in `app/gnome/src/commands.rs`.
const LOFI_SHELL_APP_ID = 'dev.jplein.LoFi.desktop';

function isLauncherWindow(win: Meta.Window | null): boolean {
    if (win === null) {
        return false;
    }
    // Primary: GTK4 Wayland path. AdwApplication sets the xdg-shell app_id
    // to the GApplication id; Mutter surfaces it here verbatim.
    if (win.get_gtk_application_id() === LOFI_GAPP_ID) {
        return true;
    }
    // X11 / non-GTK fallback. GTK derives WM_CLASS from the GApplication
    // id with the first segment lowercased and dots replaced — but the
    // exact transformation varies, so accept both the raw id and the
    // bare binary name.
    const wmClass = win.get_wm_class();
    if (wmClass === LOFI_GAPP_ID || wmClass === 'lofi') {
        return true;
    }
    // Last resort: if a desktop file is installed, Shell.WindowTracker
    // will resolve a Shell.App whose id is the .desktop filename.
    const tracker = Shell.WindowTracker.get_default();
    const app = tracker.get_window_app(win) as Shell.App | null;
    return app !== null && app.get_id() === LOFI_SHELL_APP_ID;
}

/**
 * Suppresses GNOME Shell's open/close animations for the LoFi launcher
 * window only. The launcher is modal, focus-driven, and dismisses on focus
 * loss; the default zoom/fade adds perceptible latency to a flow the user
 * expects to feel instantaneous. All other windows keep their normal
 * animations.
 *
 * Mechanism: listen on Shell.WM's `map` and `destroy` signals, which fire
 * once the actor exists and the effect has been queued by Shell. We remove
 * the in-flight transitions and snap the actor to its terminal state. This
 * is not "prevent the animation" — it's "cut its duration to zero" — but
 * the visible result is the same, and it avoids monkey-patching
 * `Main.wm._mapWindow` / `_destroyWindow`, which are private and shift
 * between GNOME versions.
 *
 * For `destroy` we also call `WM.completed_destroy(actor)` because the
 * default Shell handler runs its own animation and then signals
 * completion; with the transitions removed Shell would otherwise keep
 * the destroy gated behind a zero-duration animation it thinks is still
 * running.
 */
export class LauncherAnimationSuppressor {
    private mapHandlerId = 0;
    private destroyHandlerId = 0;

    enable(): void {
        const wm = global.window_manager;
        this.mapHandlerId = wm.connect('map', (_wm, actor) => {
            if (!isLauncherWindow(actor.meta_window)) {
                return;
            }
            actor.remove_all_transitions();
            actor.opacity = 255;
            actor.scale_x = 1;
            actor.scale_y = 1;
            actor.translation_y = 0;
        });
        this.destroyHandlerId = wm.connect('destroy', (shellwm, actor) => {
            if (!isLauncherWindow(actor.meta_window)) {
                return;
            }
            actor.remove_all_transitions();
            shellwm.completed_destroy(actor);
        });
    }

    disable(): void {
        const wm = global.window_manager;
        if (this.mapHandlerId !== 0) {
            wm.disconnect(this.mapHandlerId);
            this.mapHandlerId = 0;
        }
        if (this.destroyHandlerId !== 0) {
            wm.disconnect(this.destroyHandlerId);
            this.destroyHandlerId = 0;
        }
    }
}
