use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use lofi_core::{Entry, EntryRef, MruStore};
use lofi_linux::{apps, backend, config, ui};

/// GApplication id, which is also what makes the second `lofi` invocation a
/// remote that toggles the first (see `on_activate`). Must stay in lockstep
/// with `backend::LOFI_DESKTOP_ID` — the backends filter LoFi's own window out
/// of the command-target pick by that id — and with the installed `.desktop`
/// file's name.
const APP_ID: &str = "dev.jplein.LoFi";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(on_activate);
    app.run()
}

fn on_activate(app: &adw::Application) {
    // Toggle: a second `lofi` invocation is routed here via GApplication's
    // single-instance D-Bus dispatch (`application_id` makes the second
    // process a remote that pings the primary's `activate` and exits). If
    // a window is already up, close it and bail — `adw::Application` quits
    // when its last window closes, so the toggle is a clean open ↔ closed.
    let existing: Vec<gtk::Window> = app.windows().into_iter().collect();
    if !existing.is_empty() {
        for w in existing {
            w.close();
        }
        return;
    }

    // Read the user's configuration file once, up front. It is best-effort by
    // design — a missing file is the common case and a malformed one costs the
    // user their styling, not their launcher — so this never fails. See
    // `config`.
    let config = config::load();

    // Pick the desktop backend once, up front: everything below that isn't
    // plain XDG application enumeration goes through it, and the UI keeps a
    // handle so `launch::activate` can dispatch on Enter. See
    // `backend::detect` for how the choice is made. The config goes in because
    // the Niri backend's Lock command consults `lock-command` from it.
    let backend = backend::create(&config);

    let dirs = apps::application_directories();
    let mut applications = apps::gather_applications(&dirs);
    let windows = backend.gather_windows();
    let workspaces_vec = backend.gather_workspaces();
    let commands_vec = backend.gather_commands(&windows);
    let workspace_commands = backend.gather_workspace_commands(&windows, &workspaces_vec);
    let power_commands = backend.gather_power_commands();
    let summon_commands = backend.gather_summon_commands(&windows);

    // Build a desktop_id -> most-recent-window-id map. Every backend
    // guarantees `gather_windows` is in MRU order, so the FIRST occurrence per
    // app id is the right one — `insert` on an existing key would clobber MRU
    // with a less-recent entry, hence the let-chain guard with `contains_key`.
    let mut mru: HashMap<String, u64> = HashMap::new();
    for w in &windows {
        if let Some(id) = w.app_desktop_id.as_ref()
            && !mru.contains_key(id)
        {
            mru.insert(id.clone(), w.id);
        }
    }

    // Annotate each Application with the recent-window id we just computed.
    // `is_running` is the boolean projection — kept in lockstep here so the
    // running-indicator dot (which reads `is_running` for cross-platform
    // parity with the macOS path) and the focus-vs-launch branch in
    // `launch.rs` (which reads the window id) stay consistent.
    for app in &mut applications {
        app.recent_window_id = mru.get(&app.desktop_id).copied();
        app.is_running = app.recent_window_id.is_some();
    }

    let mut entries: Vec<Entry> = Vec::with_capacity(
        applications.len()
            + windows.len()
            + workspaces_vec.len()
            + commands_vec.len()
            + workspace_commands.len()
            + power_commands.len()
            + summon_commands.len(),
    );
    entries.extend(applications.into_iter().map(Entry::Application));
    entries.extend(windows.into_iter().map(Entry::Window));
    entries.extend(workspaces_vec.into_iter().map(Entry::Workspace));
    entries.extend(commands_vec.into_iter().map(Entry::Command));
    entries.extend(workspace_commands.into_iter().map(Entry::WorkspaceCommand));
    entries.extend(power_commands.into_iter().map(Entry::PowerCommand));
    entries.extend(summon_commands.into_iter().map(Entry::SummonWindow));

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

    ui::build(
        app,
        entries,
        mru_store,
        mru_index,
        backend,
        &config.appearance,
    );
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
