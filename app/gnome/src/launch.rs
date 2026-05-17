use gio_unix::DesktopAppInfo;
use gtk::gio::prelude::*;
use gtk::prelude::*;
use lofi_core::Entry;

use crate::windows;

/// Launch the application backing `entry` via gio. Errors are logged to stderr
/// and swallowed because there is no meaningful caller-side recovery from
/// "the desktop file vanished between gather and click".
pub fn activate(entry: &Entry) {
    match entry {
        Entry::Application(app) => {
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
    }
}
