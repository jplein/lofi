use gio_unix::DesktopAppInfo;
use gtk::gio::prelude::*;
use gtk::prelude::*;
use lofi_core::Entry;

use crate::backend::Backend;

/// Activate the entry — the single dispatch point for "the user pressed Enter
/// on this row".
///
/// The one branch that isn't the backend's business is launching an
/// application: `.desktop` activation is XDG, not window-system, so it goes
/// straight to gio on either desktop. Everything else — focusing, switching,
/// window actions, workspace moves, power — is desktop-specific and hands off
/// to `backend`, which is why this function takes one.
///
/// For an `Entry::Application` that has a `recent_window_id` (i.e. is
/// currently running), focus that window instead of launching a fresh
/// instance — mirroring the GNOME dock's "click running app = raise existing
/// window" behaviour. We deliberately do *not* fall back from focus to launch
/// when focusing fails: the gather-vs-click race is real but rare, and a
/// phantom second instance would be more surprising than a silent no-op.
///
/// Errors are logged to stderr and swallowed because there is no meaningful
/// caller-side recovery from "the desktop file vanished between gather and
/// click".
pub fn activate(backend: &dyn Backend, entry: &Entry) {
    match entry {
        Entry::Application(app) => {
            if let Some(window_id) = app.recent_window_id {
                backend.focus_window(window_id);
                return;
            }

            let info = match DesktopAppInfo::new(&app.desktop_id) {
                Some(i) => i,
                None => {
                    eprintln!("lofi: no DesktopAppInfo for {}", app.desktop_id);
                    return;
                }
            };

            // The launch context carries the launching display, so the new
            // app starts on the right monitor.
            let context = gtk::gdk::Display::default().map(|d| d.app_launch_context());

            let launch_result = info.launch(&[], context.as_ref());
            if let Err(e) = launch_result {
                eprintln!("lofi: launch failed for {}: {e}", app.desktop_id);
            }
        }
        Entry::Window(w) => backend.focus_window(w.id),
        Entry::Workspace(w) => backend.activate_workspace(w.index),
        Entry::Command(cmd) => backend.run_command(cmd),
        Entry::PowerCommand(c) => backend.run_power_command(c.kind),
        Entry::WorkspaceCommand(wc) => backend.run_workspace_command(wc),
    }
}
