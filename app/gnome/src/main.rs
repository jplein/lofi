use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use lofi_core::{Entry, EntryRef, MruStore};
use lofi_gnome::{apps, commands, ui, windows, workspaces};

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
    let workspaces_vec = workspaces::gather_workspaces();
    let commands_vec = commands::gather_commands();

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

    let mut entries: Vec<Entry> = Vec::with_capacity(
        applications.len() + windows.len() + workspaces_vec.len() + commands_vec.len(),
    );
    entries.extend(applications.into_iter().map(Entry::Application));
    entries.extend(windows.into_iter().map(Entry::Window));
    entries.extend(workspaces_vec.into_iter().map(Entry::Workspace));
    entries.extend(commands_vec.into_iter().map(Entry::Command));

    // Open the persistent MRU store and snapshot the recency index. Both are
    // best-effort: any failure (no XDG_STATE_HOME + no HOME, permission
    // denied, corrupt DB) logs and leaves the launcher with an empty index
    // so first-run / broken-environment users still get a working list.
    let mru_store = mru_state_path().and_then(|p| {
        MruStore::open(&p)
            .map_err(|e| eprintln!("mru: open failed at {}: {e}", p.display()))
            .ok()
    });
    let mru_index: Vec<EntryRef> = mru_store
        .as_ref()
        .and_then(|s| {
            s.read_all()
                .map_err(|e| eprintln!("mru: read failed: {e}"))
                .ok()
        })
        .unwrap_or_default();

    // Wrap in Rc so the activate/click closures in ui.rs can each hold a
    // clone without moving the original.
    let mru_store = mru_store.map(Rc::new);

    ui::build(app, entries, mru_store, mru_index);
}

/// Resolve the on-disk path for the MRU SQLite file. Mirrors the manual XDG
/// pattern used in `apps::application_directories`: prefer `$XDG_STATE_HOME`,
/// fall back to `$HOME/.local/state`, return `None` if neither resolves so
/// the launcher proceeds with no persistent history rather than crashing.
fn mru_state_path() -> Option<PathBuf> {
    let state_home: PathBuf = match env::var("XDG_STATE_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => match env::var("HOME") {
            Ok(home) if !home.is_empty() => {
                let mut p = PathBuf::from(home);
                p.push(".local");
                p.push("state");
                p
            }
            _ => return None,
        },
    };
    let mut path = state_home;
    path.push("lofi");
    path.push("mru.sqlite");
    Some(path)
}
