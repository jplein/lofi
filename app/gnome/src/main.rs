use std::collections::HashMap;

use adw::prelude::*;
use gtk::glib;
use lofi_core::Entry;
use lofi_gnome::{apps, ui, windows};

const APP_ID: &str = "dev.jplein.LoFi";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(on_activate);
    app.run()
}

fn on_activate(app: &adw::Application) {
    let dirs = apps::application_directories();
    let mut applications = apps::gather_applications(&dirs);
    let windows = windows::gather_windows();

    // Build a desktop_id -> most-recent-window-id map. `gather_windows` returns
    // windows in MRU order, so the FIRST occurrence per app id is the right
    // one — `insert` on an existing key would clobber MRU with a less-recent
    // entry, hence the let-chain guard with `contains_key`.
    let mut mru: HashMap<String, u64> = HashMap::new();
    for w in &windows {
        if let Some(id) = w.app_desktop_id.as_ref()
            && !mru.contains_key(id)
        {
            mru.insert(id.clone(), w.id);
        }
    }

    // Annotate each Application with the recent-window id we just computed.
    // This is what drives both the running-indicator dot in `ui.rs` and the
    // focus-vs-launch branch in `launch.rs`.
    for app in &mut applications {
        app.recent_window_id = mru.get(&app.desktop_id).copied();
    }

    let mut entries: Vec<Entry> = Vec::with_capacity(applications.len() + windows.len());
    entries.extend(applications.into_iter().map(Entry::Application));
    entries.extend(windows.into_iter().map(Entry::Window));
    ui::build(app, entries);
}
