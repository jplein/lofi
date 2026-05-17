import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import type Meta from 'gi://Meta';

import { dbusXml } from './dbus-xml.js';
import * as windows from './windows.js';
import * as workspaces from './workspaces.js';
import * as displays from './displays.js';
import {
    windowNotFound,
    workspaceOutOfRange,
} from './errors.js';

const BUS_NAME = 'dev.jplein.LoFi.Shell';
const OBJECT_PATH = '/dev/jplein/LoFi/Shell';
const MUTTER_SCHEMA_ID = 'org.gnome.mutter';
const DYNAMIC_WORKSPACES_KEY = 'dynamic-workspaces';

function dynamicWorkspacesEnabled(): boolean {
    try {
        const settings = new Gio.Settings({ schema_id: MUTTER_SCHEMA_ID });
        return settings.get_boolean(DYNAMIC_WORKSPACES_KEY);
    } catch {
        return false;
    }
}

function lookupWindow(id: bigint | number): Meta.Window {
    const win = windows.byId(id);
    if (win === null) {
        throw windowNotFound(id);
    }
    return win;
}

function moveWindowToWorkspaceIndex(
    win: Meta.Window,
    target: number,
    follow: boolean,
): void {
    if (win.is_on_all_workspaces()) {
        win.unstick();
    }
    win.change_workspace_by_index(target, follow);
}

export class WindowManagerService {
    private exported: Gio.DBusExportedObject | null = null;
    private busOwnerId = 0;

    export(): void {
        const exported = Gio.DBusExportedObject.wrapJSObject(dbusXml, this);
        exported.export(
            Gio.bus_get_sync(Gio.BusType.SESSION, null),
            OBJECT_PATH,
        );
        this.exported = exported;
        this.busOwnerId = Gio.bus_own_name(
            Gio.BusType.SESSION,
            BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            null,
            null,
        );
    }

    unexport(): void {
        if (this.busOwnerId !== 0) {
            Gio.bus_unown_name(this.busOwnerId);
            this.busOwnerId = 0;
        }
        if (this.exported !== null) {
            this.exported.unexport();
            this.exported = null;
        }
    }

    // ---- read methods ----

    ListWindows(): Record<string, GLib.Variant>[] {
        return windows.list();
    }

    ListWindowsMRU(): Record<string, GLib.Variant>[] {
        return windows.listMRU();
    }

    GetActiveWindow(): Record<string, GLib.Variant> {
        return windows.active() ?? {};
    }

    GetWindowWorkArea(id: bigint): Record<string, GLib.Variant> {
        const win = lookupWindow(id);
        const rect = win.get_work_area_current_monitor();
        return {
            x: GLib.Variant.new_int32(rect.x),
            y: GLib.Variant.new_int32(rect.y),
            width: GLib.Variant.new_int32(rect.width),
            height: GLib.Variant.new_int32(rect.height),
        };
    }

    GetWindowFrame(id: bigint): Record<string, GLib.Variant> {
        const win = lookupWindow(id);
        const rect = win.get_frame_rect();
        return {
            x: GLib.Variant.new_int32(rect.x),
            y: GLib.Variant.new_int32(rect.y),
            width: GLib.Variant.new_int32(rect.width),
            height: GLib.Variant.new_int32(rect.height),
        };
    }

    ListWorkspaces(): Record<string, GLib.Variant>[] {
        return workspaces.list();
    }

    ListDisplays(): Record<string, GLib.Variant>[] {
        return displays.list();
    }

    GetActiveDisplay(): Record<string, GLib.Variant> {
        return displays.active();
    }

    // ---- by-id actions ----

    FocusWindow(id: bigint): void {
        const win = lookupWindow(id);
        win.activate(global.display.get_current_time_roundtrip());
    }

    MoveWindowToWorkspace(id: bigint, targetIndex: number): void {
        const win = lookupWindow(id);
        const wm = global.workspace_manager;

        if (targetIndex < 0) {
            throw workspaceOutOfRange(targetIndex);
        }

        const dynamic = dynamicWorkspacesEnabled();
        if (dynamic) {
            if (targetIndex > wm.n_workspaces) {
                throw workspaceOutOfRange(targetIndex);
            }
            if (targetIndex === wm.n_workspaces) {
                wm.append_new_workspace(false, global.display.get_current_time_roundtrip());
            }
        } else if (targetIndex >= wm.n_workspaces) {
            throw workspaceOutOfRange(targetIndex);
        }

        moveWindowToWorkspaceIndex(win, targetIndex, false);
    }

    MoveResizeWindow(
        id: bigint,
        x: number,
        y: number,
        width: number,
        height: number,
    ): void {
        const win = lookupWindow(id);
        // Every caller of MoveResizeWindow is a geometry command (center,
        // half-width, etc.) that conceptually replaces the window's current
        // state with a precise rectangle. Mutter ignores `move_resize_frame`
        // on a maximized or fullscreen window, so we unmaximize/unfullscreen
        // first to make the call effective in those states. Matches the
        // original window-commands set, which did the same thing.
        if (win.is_fullscreen()) {
            win.unmake_fullscreen();
        }
        win.unmaximize();
        win.move_resize_frame(true, x, y, width, height);
    }

    MinimizeWindow(id: bigint): void {
        const win = lookupWindow(id);
        win.minimize();
    }

    ToggleMaximizeWindow(id: bigint): void {
        const win = lookupWindow(id);
        if (win.is_maximized()) {
            win.unmaximize();
        } else {
            win.maximize();
        }
    }

    ToggleFullscreenWindow(id: bigint): void {
        const win = lookupWindow(id);
        if (win.is_fullscreen()) {
            win.unmake_fullscreen();
        } else {
            win.make_fullscreen();
        }
    }

    CloseWindow(id: bigint): void {
        const win = lookupWindow(id);
        win.delete(global.display.get_current_time_roundtrip());
    }

    // ---- workspace action ----

    ActivateWorkspace(index: number): void {
        const wm = global.workspace_manager;
        if (index < 0 || index >= wm.n_workspaces) {
            throw workspaceOutOfRange(index);
        }
        const ws = wm.get_workspace_by_index(index);
        if (ws === null) {
            throw workspaceOutOfRange(index);
        }
        ws.activate(global.display.get_current_time_roundtrip());
    }
}
