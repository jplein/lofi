use gio_unix::DesktopAppInfo;
use gtk::gio::prelude::*;
use gtk::prelude::*;
use lofi_core::{CommandKind, Entry, compute_geometry};

use crate::{windows, workspaces};

/// Activate the entry. For an `Entry::Application` that has a
/// `recent_window_id` (i.e. is currently running), focus that window over
/// D-Bus instead of launching a fresh instance — mirroring the GNOME dock's
/// "click running app = raise existing window" behaviour. Otherwise launch
/// via gio. Window entries focus by id as before. Errors are logged to
/// stderr and swallowed because there is no meaningful caller-side recovery
/// from "the desktop file vanished between gather and click".
pub fn activate(entry: &Entry) {
    match entry {
        Entry::Application(app) => {
            if let Some(window_id) = app.recent_window_id {
                windows::focus_window(window_id);
                return;
            }

            let info = match DesktopAppInfo::new(&app.desktop_id) {
                Some(i) => i,
                None => {
                    eprintln!("lofi: no DesktopAppInfo for {}", app.desktop_id);
                    return;
                }
            };

            let context = gtk::gdk::Display::default().map(|d| d.app_launch_context());

            let launch_result = info.launch(&[], context.as_ref());
            if let Err(e) = launch_result {
                eprintln!("lofi: launch failed for {}: {e}", app.desktop_id);
            }
        }
        Entry::Window(w) => {
            windows::focus_window(w.id);
        }
        Entry::Workspace(w) => {
            workspaces::activate_workspace(w.index);
        }
        Entry::Command(cmd) => {
            let id = cmd.target_window_id;
            match cmd.kind {
                CommandKind::Minimize => windows::minimize_window(id),
                CommandKind::ToggleMaximize => windows::toggle_maximize_window(id),
                CommandKind::ToggleFullscreen => windows::toggle_fullscreen_window(id),
                kind => {
                    if let Some((x, y, w, h)) =
                        compute_geometry(kind, &cmd.work_area, cmd.current_frame)
                    {
                        windows::move_resize_window(id, x, y, w, h);
                    }
                }
            }
        }
    }
}
