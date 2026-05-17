import type Meta from 'gi://Meta';
import Shell from 'gi://Shell';

/// Canonical Shell.App id of the launcher window. Derived from the desktop
/// file name (`dev.jplein.LoFi.desktop`), which is also the value
/// `app/gnome/src/commands.rs` uses as `LOFI_DESKTOP_ID` to identify the
/// launcher elsewhere. Keep both in lockstep if the desktop id ever changes.
const LOFI_APP_ID = 'dev.jplein.LoFi.desktop';

function isLauncherWindow(win: Meta.Window | null): boolean {
    if (win === null) {
        return false;
    }
    const tracker = Shell.WindowTracker.get_default();
    const app = tracker.get_window_app(win) as Shell.App | null;
    if (app === null) {
        return false;
    }
    return app.get_id() === LOFI_APP_ID;
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
