import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import type Meta from 'gi://Meta';

import { dbusXml } from './dbus-xml.js';
import * as windows from './windows.js';
import * as workspaces from './workspaces.js';
import * as displays from './displays.js';
import {
    noActiveWindow,
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

function focusWindow(): Meta.Window {
    const win = global.display.focus_window;
    if (win === null) {
        throw noActiveWindow();
    }
    return win;
}

function lookupWindow(id: bigint | number): Meta.Window {
    const win = windows.byId(id);
    if (win === null) {
        throw windowNotFound(id);
    }
    return win;
}

function currentWorkspaceIndexFor(win: Meta.Window): number {
    const wm = global.workspace_manager;
    if (win.is_on_all_workspaces()) {
        return wm.get_active_workspace_index();
    }
    const ws = win.get_workspace();
    if (ws === null) {
        return wm.get_active_workspace_index();
    }
    return ws.index();
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

    GetActiveWindow(): Record<string, GLib.Variant> {
        return windows.active() ?? {};
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

    // ---- active-window actions ----

    MoveActiveWindowToNextWorkspace(): void {
        const win = focusWindow();
        const wm = global.workspace_manager;
        const current = currentWorkspaceIndexFor(win);
        const target = current + 1;
        if (target >= wm.n_workspaces) {
            if (!dynamicWorkspacesEnabled()) {
                return;
            }
            wm.append_new_workspace(false, global.get_current_time());
        }
        moveWindowToWorkspaceIndex(win, target, true);
    }

    MoveActiveWindowToPreviousWorkspace(): void {
        const win = focusWindow();
        const current = currentWorkspaceIndexFor(win);
        const target = current - 1;
        if (target < 0) {
            return;
        }
        moveWindowToWorkspaceIndex(win, target, true);
    }

    MoveResizeActiveWindow(
        x: number,
        y: number,
        width: number,
        height: number,
    ): void {
        const win = focusWindow();
        win.move_resize_frame(true, x, y, width, height);
    }

    MaximizeActiveWindow(): void {
        const win = focusWindow();
        win.maximize();
    }

    UnmaximizeActiveWindow(): void {
        const win = focusWindow();
        win.unmaximize();
    }

    // ---- by-id actions ----

    FocusWindow(id: bigint): void {
        const win = lookupWindow(id);
        win.activate(global.get_current_time());
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
                wm.append_new_workspace(false, global.get_current_time());
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
        win.move_resize_frame(true, x, y, width, height);
    }

    CloseWindow(id: bigint): void {
        const win = lookupWindow(id);
        win.delete(global.get_current_time());
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
        ws.activate(global.get_current_time());
    }
}
