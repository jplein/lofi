import GLib from 'gi://GLib';
import type Meta from 'gi://Meta';
import Shell from 'gi://Shell';

export type WindowDict = Record<string, GLib.Variant>;

function windowActors(): Meta.WindowActor[] {
    return global.get_window_actors();
}

function metaWindows(): Meta.Window[] {
    const result: Meta.Window[] = [];
    for (const actor of windowActors()) {
        const win = actor.meta_window;
        if (win !== null && !win.is_override_redirect()) {
            result.push(win);
        }
    }
    return result;
}

function appIdFor(win: Meta.Window): string {
    const tracker = Shell.WindowTracker.get_default();
    const app = tracker.get_window_app(win);
    // get_window_app can return null in practice; the typing is App but
    // the runtime sometimes returns null for system windows.
    const maybeApp = app as Shell.App | null;
    if (maybeApp === null) {
        return '';
    }
    const id = maybeApp.get_id();
    return id ?? '';
}

interface AppInfo {
    name: string;
    icon: string;
}

/**
 * Look up the Shell.App backing `win` and return its display name + icon.
 * Falls back to empty strings (which the Rust side coerces to `None`) when
 * Shell.WindowTracker has no app for the window — same defensive pattern as
 * `appIdFor`.
 */
function resolveAppInfo(win: Meta.Window): AppInfo {
    const tracker = Shell.WindowTracker.get_default();
    const app = tracker.get_window_app(win);
    const maybeApp = app as Shell.App | null;
    if (maybeApp === null) {
        return { name: '', icon: '' };
    }
    const name = maybeApp.get_name() ?? '';
    const gicon = maybeApp.get_icon();
    const icon = gicon === null ? '' : gicon.to_string() ?? '';
    return { name, icon };
}

function workspaceIndex(win: Meta.Window): number {
    if (win.is_on_all_workspaces()) {
        return -1;
    }
    const ws = win.get_workspace();
    if (ws === null) {
        return -1;
    }
    return ws.index();
}

export function serialize(win: Meta.Window): WindowDict {
    const rect = win.get_frame_rect();
    const focused = win === global.display.focus_window;
    const maximized =
        win.maximized_horizontally && win.maximized_vertically;
    const info = resolveAppInfo(win);
    const dict: WindowDict = {
        id: GLib.Variant.new_uint64(win.get_id()),
        title: GLib.Variant.new_string(win.get_title() ?? ''),
        app_id: GLib.Variant.new_string(appIdFor(win)),
        app_name: GLib.Variant.new_string(info.name),
        icon: GLib.Variant.new_string(info.icon),
        workspace: GLib.Variant.new_int32(workspaceIndex(win)),
        monitor: GLib.Variant.new_int32(win.get_monitor()),
        x: GLib.Variant.new_int32(rect.x),
        y: GLib.Variant.new_int32(rect.y),
        width: GLib.Variant.new_int32(rect.width),
        height: GLib.Variant.new_int32(rect.height),
        focused: GLib.Variant.new_boolean(focused),
        minimized: GLib.Variant.new_boolean(win.minimized),
        maximized: GLib.Variant.new_boolean(maximized),
        fullscreen: GLib.Variant.new_boolean(win.is_fullscreen()),
        on_all_workspaces: GLib.Variant.new_boolean(win.is_on_all_workspaces()),
    };
    return dict;
}

export function list(): WindowDict[] {
    return metaWindows().map(serialize);
}

export function active(): WindowDict | null {
    const win = global.display.focus_window;
    if (win === null) {
        return null;
    }
    return serialize(win);
}

export function byId(id: bigint | number): Meta.Window | null {
    const target = BigInt(id);
    for (const actor of windowActors()) {
        const win = actor.meta_window;
        if (win === null) {
            continue;
        }
        if (BigInt(win.get_id()) === target) {
            return win;
        }
    }
    return null;
}
