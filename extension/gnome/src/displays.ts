import GLib from 'gi://GLib';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

export type DisplayDict = Record<string, GLib.Variant>;

function activeMonitorIndex(): number {
    const focusWin = global.display.focus_window;
    if (focusWin !== null) {
        return focusWin.get_monitor();
    }
    return global.display.get_current_monitor();
}

function buildDict(index: number, activeIdx: number): DisplayDict {
    const monitor = Main.layoutManager.monitors[index];
    if (monitor === undefined) {
        return {
            index: GLib.Variant.new_int32(index),
            name: GLib.Variant.new_string(`Monitor ${index + 1}`),
            x: GLib.Variant.new_int32(0),
            y: GLib.Variant.new_int32(0),
            width: GLib.Variant.new_int32(0),
            height: GLib.Variant.new_int32(0),
            scale: GLib.Variant.new_double(1.0),
            primary: GLib.Variant.new_boolean(false),
            active: GLib.Variant.new_boolean(false),
        };
    }
    const primaryIdx = global.display.get_primary_monitor();
    const scale = global.display.get_monitor_scale(index);
    return {
        index: GLib.Variant.new_int32(index),
        name: GLib.Variant.new_string(`Monitor ${index + 1}`),
        x: GLib.Variant.new_int32(monitor.x),
        y: GLib.Variant.new_int32(monitor.y),
        width: GLib.Variant.new_int32(monitor.width),
        height: GLib.Variant.new_int32(monitor.height),
        scale: GLib.Variant.new_double(scale),
        primary: GLib.Variant.new_boolean(index === primaryIdx),
        active: GLib.Variant.new_boolean(index === activeIdx),
    };
}

export function list(): DisplayDict[] {
    const monitors = Main.layoutManager.monitors;
    const activeIdx = activeMonitorIndex();
    const result: DisplayDict[] = [];
    for (let i = 0; i < monitors.length; i++) {
        result.push(buildDict(i, activeIdx));
    }
    return result;
}

export function active(): DisplayDict {
    const activeIdx = activeMonitorIndex();
    return buildDict(activeIdx, activeIdx);
}
