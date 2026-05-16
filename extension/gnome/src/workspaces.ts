import GLib from 'gi://GLib';

export type WorkspaceDict = Record<string, GLib.Variant>;

export function list(): WorkspaceDict[] {
    const wm = global.workspace_manager;
    const activeIndex = wm.get_active_workspace_index();
    const result: WorkspaceDict[] = [];
    for (let i = 0; i < wm.n_workspaces; i++) {
        const ws = wm.get_workspace_by_index(i);
        const nWindows = ws !== null ? ws.list_windows().length : 0;
        result.push({
            index: GLib.Variant.new_int32(i),
            name: GLib.Variant.new_string(`Workspace ${i + 1}`),
            active: GLib.Variant.new_boolean(i === activeIndex),
            n_windows: GLib.Variant.new_int32(nWindows),
        });
    }
    return result;
}

export function active(): WorkspaceDict | null {
    const wm = global.workspace_manager;
    const idx = wm.get_active_workspace_index();
    if (idx < 0 || idx >= wm.n_workspaces) {
        return null;
    }
    const ws = wm.get_workspace_by_index(idx);
    const nWindows = ws !== null ? ws.list_windows().length : 0;
    return {
        index: GLib.Variant.new_int32(idx),
        name: GLib.Variant.new_string(`Workspace ${idx + 1}`),
        active: GLib.Variant.new_boolean(true),
        n_windows: GLib.Variant.new_int32(nWindows),
    };
}
