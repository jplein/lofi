import GLib from 'gi://GLib';

const ERROR_DOMAIN_PREFIX = 'dev.jplein.LoFi.Shell.Error';

function dbusError(name: string, message: string): GLib.Error {
    return GLib.Error.new_literal(
        GLib.quark_from_string(`${ERROR_DOMAIN_PREFIX}.${name}`),
        0,
        message,
    );
}

export function noActiveWindow(): GLib.Error {
    return dbusError('NoActiveWindow', 'No active window');
}

export function windowNotFound(id: bigint | number): GLib.Error {
    return dbusError('WindowNotFound', `Window not found: ${id}`);
}

export function workspaceOutOfRange(idx: number): GLib.Error {
    return dbusError(
        'WorkspaceOutOfRange',
        `Workspace index out of range: ${idx}`,
    );
}
