use crate::Entry;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Build the searchable text for an entry. Exhaustive on `Entry` so adding a
/// variant is a compile error until this is updated.
fn haystack(entry: &Entry) -> String {
    match entry {
        Entry::Application(app) => format!("{} {}", app.name, app.desktop_id),
        Entry::Window(w) => match &w.app_name {
            Some(app) => format!("{} {}", w.title, app),
            None => w.title.clone(),
        },
        Entry::Workspace(w) => w.name.clone(),
        Entry::Command(c) => c.kind.display_name().to_string(),
        Entry::PowerCommand(c) => c.kind.display_name().to_string(),
    }
}

/// Fuzzy-filter `entries` by `query`. An empty or whitespace-only query is a
/// passthrough that returns every entry. Otherwise the query is tokenized on
/// whitespace and every token must match the entry's haystack (intersection
/// semantics). Results preserve the input order — `search` is filter-only;
/// ordering is the caller's responsibility (the launcher uses the MRU index).
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    if query.trim().is_empty() {
        return entries.iter().collect();
    }

    let tokens: Vec<&str> = query.split_whitespace().collect();
    let matcher = SkimMatcherV2::default().ignore_case();

    entries
        .iter()
        .filter(|entry| {
            let hay = haystack(entry);
            tokens
                .iter()
                .all(|token| matcher.fuzzy_match(&hay, token).is_some())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Application, Command, CommandKind, Entry, PowerCommand, PowerCommandKind, Window, WorkArea,
        Workspace,
    };
    use std::collections::HashSet;

    /// Test helper: build an `Entry::Application` with the given name/desktop_id
    /// and no icon. Kept terse so fixtures stay readable.
    fn app(name: &str, desktop_id: &str) -> Entry {
        Entry::Application(Application {
            name: name.into(),
            desktop_id: desktop_id.into(),
            icon: None,
            recent_window_id: None,
        })
    }

    /// Test helper: build an `Entry::Window` with the given id/title/app_name and
    /// no icon on workspace 0. Mirrors `app(...)` for window fixtures.
    fn win(id: u64, title: &str, app_name: Option<&str>) -> Entry {
        Entry::Window(Window {
            id,
            title: title.into(),
            app_name: app_name.map(str::to_string),
            icon: None,
            workspace: 0,
            app_desktop_id: None,
        })
    }

    /// Test helper: build an `Entry::Workspace` with the given index and name.
    /// Mirrors `app(...)` and `win(...)` for workspace fixtures.
    fn workspace(index: i32, name: &str) -> Entry {
        Entry::Workspace(Workspace {
            index,
            name: name.into(),
        })
    }

    /// Test helper: build an `Entry::Command` with the given kind. The
    /// matcher reads only the `kind.display_name()`, so the rest of the fields
    /// are filled with placeholder values.
    fn cmd(kind: CommandKind) -> Entry {
        Entry::Command(Command {
            kind,
            target_window_id: 1,
            work_area: WorkArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            current_frame: (0, 0, 0, 0),
        })
    }

    /// Test helper: build an `Entry::PowerCommand` with the given kind. The
    /// matcher reads only the `kind.display_name()`, so no other state is
    /// needed. Mirrors `cmd(...)` for power-command fixtures.
    fn power(kind: PowerCommandKind) -> Entry {
        Entry::PowerCommand(PowerCommand { kind })
    }

    /// Collect names out of a `Vec<&Entry>` for set-based or order-based assertions.
    fn names<'a>(results: &'a [&'a Entry]) -> Vec<&'a str> {
        results.iter().map(|e| e.name()).collect()
    }

    #[test]
    fn empty_query_returns_all_entries_in_input_order() {
        let entries = vec![
            app("Alpha", "alpha.desktop"),
            app("Bravo", "bravo.desktop"),
            app("Charlie", "charlie.desktop"),
        ];

        let result = search(&entries, "");
        assert_eq!(
            result.len(),
            entries.len(),
            "empty query should return all entries; got {} of {}",
            result.len(),
            entries.len()
        );
        assert_eq!(
            names(&result),
            vec!["Alpha", "Bravo", "Charlie"],
            "empty query should preserve input order; got {:?}",
            names(&result)
        );

        let ws_result = search(&entries, "   ");
        assert_eq!(
            ws_result.len(),
            entries.len(),
            "whitespace-only query should return all entries; got {}",
            ws_result.len()
        );
        assert_eq!(
            names(&ws_result),
            vec!["Alpha", "Bravo", "Charlie"],
            "whitespace-only query should preserve input order; got {:?}",
            names(&ws_result)
        );
    }

    #[test]
    fn single_token_matches_name_case_insensitively() {
        let entries = vec![
            app("Firefox", "firefox.desktop"),
            app("Files", "org.gnome.Nautilus.desktop"),
            app("Chromium", "chromium.desktop"),
        ];

        let result = search(&entries, "FIRE");
        let name_set: HashSet<&str> = result.iter().map(|e| e.name()).collect();

        assert!(
            name_set.contains("Firefox"),
            "query \"FIRE\" should match \"Firefox\" case-insensitively; got names {:?}",
            name_set
        );
        assert!(
            name_set.contains("Files"),
            "query \"FIRE\" should fuzzy-match \"Files\" (F-I-(l)-(e)-...); got names {:?}",
            name_set
        );
        assert!(
            !name_set.contains("Chromium"),
            "query \"FIRE\" should not match \"Chromium\"; got names {:?}",
            name_set
        );
    }

    #[test]
    fn single_token_matches_desktop_id() {
        let entries = vec![
            app("Chrome", "com.google.Chrome.desktop"),
            app("Maps", "org.gnome.Maps.desktop"),
            app("Firefox", "firefox.desktop"),
        ];

        let result = search(&entries, "google");

        assert_eq!(
            result.len(),
            1,
            "query \"google\" should match exactly one entry by desktop_id; got names {:?}",
            names(&result)
        );
        assert!(
            result.iter().any(|e| e.name() == "Chrome"),
            "query \"google\" should match the Chrome entry (com.google.Chrome.desktop); got names {:?}",
            names(&result)
        );
    }

    #[test]
    fn multi_token_is_order_independent_and_intersects() {
        let entries = vec![
            app("Google Chrome", "com.google.Chrome.desktop"),
            app("Google Earth", "com.google.Earth.desktop"),
            app("Firefox", "firefox.desktop"),
        ];

        let forward = search(&entries, "chr goo");
        let reverse = search(&entries, "goo chr");

        let forward_names: HashSet<&str> = forward.iter().map(|e| e.name()).collect();
        let reverse_names: HashSet<&str> = reverse.iter().map(|e| e.name()).collect();

        assert!(
            forward_names.contains("Google Chrome"),
            "\"chr goo\" should match \"Google Chrome\"; got {:?}",
            forward_names
        );
        assert!(
            !forward_names.contains("Google Earth"),
            "\"chr goo\" should not match \"Google Earth\" (no \"chr\" in name or id); got {:?}",
            forward_names
        );
        assert!(
            !forward_names.contains("Firefox"),
            "\"chr goo\" should not match \"Firefox\" (no \"goo\"); got {:?}",
            forward_names
        );

        assert!(
            reverse_names.contains("Google Chrome"),
            "\"goo chr\" should match \"Google Chrome\"; got {:?}",
            reverse_names
        );
        assert!(
            !reverse_names.contains("Google Earth"),
            "\"goo chr\" should not match \"Google Earth\"; got {:?}",
            reverse_names
        );
        assert!(
            !reverse_names.contains("Firefox"),
            "\"goo chr\" should not match \"Firefox\"; got {:?}",
            reverse_names
        );

        let mut forward_sorted: Vec<String> =
            forward.iter().map(|e| e.name().to_string()).collect();
        let mut reverse_sorted: Vec<String> =
            reverse.iter().map(|e| e.name().to_string()).collect();
        forward_sorted.sort();
        reverse_sorted.sort();
        assert_eq!(
            forward_sorted, reverse_sorted,
            "token order should not affect the matched set; \"chr goo\" -> {:?}, \"goo chr\" -> {:?}",
            forward_sorted, reverse_sorted
        );
    }

    #[test]
    fn matching_entries_are_returned_regardless_of_order() {
        // search() is filter-only now: it returns matching entries in the
        // input order (no score-based ranking, no tiebreaker). The set of
        // matches is what callers should rely on; ordering is handled
        // upstream by the MRU index.
        let entries = vec![app("Bravo", "z.desktop"), app("Alpha", "z.desktop")];

        let result = search(&entries, "z.desktop");

        assert_eq!(
            result.len(),
            2,
            "both entries should match \"z.desktop\"; got names {:?}",
            names(&result)
        );

        assert!(
            result.iter().any(|e| e.name() == "Alpha"),
            "result should contain \"Alpha\"; got names {:?}",
            names(&result)
        );
        assert!(
            result.iter().any(|e| e.name() == "Bravo"),
            "result should contain \"Bravo\"; got names {:?}",
            names(&result)
        );
    }

    #[test]
    fn non_matching_query_returns_empty() {
        let entries = vec![
            app("Firefox", "firefox.desktop"),
            app("Chromium", "chromium.desktop"),
            app("Files", "org.gnome.Nautilus.desktop"),
        ];

        let result = search(&entries, "qqqqq");
        assert!(
            result.is_empty(),
            "query \"qqqqq\" should match nothing; got names {:?}",
            names(&result)
        );

        let empty: [Entry; 0] = [];
        let empty_result = search(&empty, "anything");
        assert!(
            empty_result.is_empty(),
            "searching an empty slice should return an empty result; got {} entries",
            empty_result.len()
        );
    }

    #[test]
    fn matcher_finds_window_by_title() {
        let entries = vec![
            app("Settings", "settings.desktop"),
            win(1, "GitHub — Pull Requests", Some("Firefox")),
        ];

        let result = search(&entries, "pull");

        assert!(
            result.iter().any(|e| matches!(
                e,
                Entry::Window(w) if w.title == "GitHub — Pull Requests"
            )),
            "query \"pull\" should match the GitHub Pull Requests window; got names {:?}",
            names(&result)
        );
        assert!(
            !result
                .iter()
                .any(|e| matches!(e, Entry::Application(a) if a.name == "Settings")),
            "query \"pull\" should not match the Settings application; got names {:?}",
            names(&result)
        );
    }

    #[test]
    fn matcher_finds_window_by_app_name() {
        let entries = vec![
            win(1, "Home", Some("Firefox")),
            win(2, "Inbox", Some("Thunderbird")),
        ];

        let result = search(&entries, "firefox");

        assert!(
            result
                .iter()
                .any(|e| matches!(e, Entry::Window(w) if w.title == "Home")),
            "query \"firefox\" should match the Firefox window titled \"Home\"; got names {:?}",
            names(&result)
        );
        assert!(
            !result
                .iter()
                .any(|e| matches!(e, Entry::Window(w) if w.title == "Inbox")),
            "query \"firefox\" should not match the Thunderbird window titled \"Inbox\"; got names {:?}",
            names(&result)
        );
    }

    #[test]
    fn matcher_window_with_no_app_name_matches_title_only() {
        // Exercises the `None` arm of the haystack match in matcher.rs;
        // must not panic and must match the title alone.
        let entries = vec![win(1, "Untitled", None)];

        let title_hit = search(&entries, "untitled");
        assert!(
            !title_hit.is_empty(),
            "query \"untitled\" should match a Window whose title is \"Untitled\" even with no app_name; got names {:?}",
            names(&title_hit)
        );

        let miss = search(&entries, "firefox");
        assert!(
            miss.is_empty(),
            "query \"firefox\" should not match a Window with no app_name and an unrelated title; got names {:?}",
            names(&miss)
        );
    }

    #[test]
    fn matcher_finds_workspace_by_name() {
        // Includes an Application whose name contains "workspaces" so we can
        // sanity-check that it doesn't shadow Workspace entries. The matcher
        // may or may not return that Application; what matters is that the
        // expected Workspace entries are present.
        let entries = vec![
            workspace(0, "Workspace 1"),
            workspace(1, "Workspace 2"),
            app("Workspaces App", "workspaces-app.desktop"),
        ];

        // Query "workspace 2": there must be exactly one Workspace entry in
        // the result set and it must be the one named "Workspace 2".
        let result = search(&entries, "workspace 2");
        let workspace_matches: Vec<&&Entry> = result
            .iter()
            .filter(|e| matches!(e, Entry::Workspace(_)))
            .collect();
        assert_eq!(
            workspace_matches.len(),
            1,
            "query \"workspace 2\" should match exactly one Workspace entry; got names {:?}",
            names(&result)
        );
        assert_eq!(
            workspace_matches[0].name(),
            "Workspace 2",
            "query \"workspace 2\" should match the Workspace named \"Workspace 2\"; got {:?}",
            workspace_matches[0].name()
        );

        // Query "2": "Workspace 2" must be in the result, "Workspace 1" must not.
        let result_two = search(&entries, "2");
        let names_two: HashSet<&str> = result_two.iter().map(|e| e.name()).collect();
        assert!(
            names_two.contains("Workspace 2"),
            "query \"2\" should match \"Workspace 2\"; got names {:?}",
            names_two
        );
        assert!(
            !names_two.contains("Workspace 1"),
            "query \"2\" should not match \"Workspace 1\"; got names {:?}",
            names_two
        );
    }

    #[test]
    fn matcher_finds_command_by_name() {
        // The matcher's haystack for Entry::Command is the kind's display
        // name. Tests assert about the set of Command entries matched per
        // query — Applications in the mix may or may not also match, but
        // they should never shadow the expected Command matches.
        let entries = vec![
            cmd(CommandKind::Center),
            cmd(CommandKind::CenterHalf),
            cmd(CommandKind::LeftHalf),
            cmd(CommandKind::ToggleMaximize),
            cmd(CommandKind::ToggleFullscreen),
            app("Left Hand Inc", "left-hand.desktop"),
        ];

        // Query "center" should match Center and CenterHalf (both display
        // names contain "Center"). CenterTwoThirds isn't in this fixture.
        let result_center = search(&entries, "center");
        let command_kinds_center: HashSet<CommandKind> = result_center
            .iter()
            .filter_map(|e| match e {
                Entry::Command(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert!(
            command_kinds_center.contains(&CommandKind::Center),
            "query \"center\" should match Command::Center; got {command_kinds_center:?}"
        );
        assert!(
            command_kinds_center.contains(&CommandKind::CenterHalf),
            "query \"center\" should match Command::CenterHalf; got {command_kinds_center:?}"
        );

        // Query "left" should match LeftHalf. The "Left Hand Inc" application
        // may also match — that's fine; we only assert about the command set.
        let result_left = search(&entries, "left");
        let command_kinds_left: HashSet<CommandKind> = result_left
            .iter()
            .filter_map(|e| match e {
                Entry::Command(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert!(
            command_kinds_left.contains(&CommandKind::LeftHalf),
            "query \"left\" should match Command::LeftHalf; got {command_kinds_left:?}"
        );

        // Query "toggle" should match both ToggleMaximize and ToggleFullscreen.
        let result_toggle = search(&entries, "toggle");
        let command_kinds_toggle: HashSet<CommandKind> = result_toggle
            .iter()
            .filter_map(|e| match e {
                Entry::Command(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert!(
            command_kinds_toggle.contains(&CommandKind::ToggleMaximize),
            "query \"toggle\" should match Command::ToggleMaximize; got {command_kinds_toggle:?}"
        );
        assert!(
            command_kinds_toggle.contains(&CommandKind::ToggleFullscreen),
            "query \"toggle\" should match Command::ToggleFullscreen; got {command_kinds_toggle:?}"
        );
    }

    #[test]
    fn matcher_finds_power_command_by_name() {
        // The matcher's haystack for Entry::PowerCommand is the kind's
        // display name. Tests assert about the set of PowerCommand entries
        // matched per query — Applications in the mix may or may not also
        // match, but they should never shadow the expected PowerCommand
        // matches. The "Lockheed Martin" application is here as a sanity
        // check that it doesn't displace the Lock PowerCommand.
        let entries = vec![
            power(PowerCommandKind::LockSession),
            power(PowerCommandKind::Suspend),
            power(PowerCommandKind::Restart),
            power(PowerCommandKind::Shutdown),
            app("Lockheed Martin", "lockheed.desktop"),
        ];

        // Query "lock" should match LockSession in the PowerCommand subset.
        // The "Lockheed Martin" application may or may not match — we only
        // assert about the PowerCommand subset.
        let result_lock = search(&entries, "lock");
        let power_kinds_lock: HashSet<PowerCommandKind> = result_lock
            .iter()
            .filter_map(|e| match e {
                Entry::PowerCommand(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert!(
            power_kinds_lock.contains(&PowerCommandKind::LockSession),
            "query \"lock\" should match PowerCommand::LockSession; got {power_kinds_lock:?}"
        );

        // Query "suspend" should match Suspend only among PowerCommands.
        let result_suspend = search(&entries, "suspend");
        let power_kinds_suspend: HashSet<PowerCommandKind> = result_suspend
            .iter()
            .filter_map(|e| match e {
                Entry::PowerCommand(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert!(
            power_kinds_suspend.contains(&PowerCommandKind::Suspend),
            "query \"suspend\" should match PowerCommand::Suspend; got {power_kinds_suspend:?}"
        );
        assert!(
            !power_kinds_suspend.contains(&PowerCommandKind::LockSession),
            "query \"suspend\" should NOT match PowerCommand::LockSession; got {power_kinds_suspend:?}"
        );
        assert!(
            !power_kinds_suspend.contains(&PowerCommandKind::Restart),
            "query \"suspend\" should NOT match PowerCommand::Restart; got {power_kinds_suspend:?}"
        );
        assert!(
            !power_kinds_suspend.contains(&PowerCommandKind::Shutdown),
            "query \"suspend\" should NOT match PowerCommand::Shutdown; got {power_kinds_suspend:?}"
        );

        // Query "restart" should match Restart only among PowerCommands.
        let result_restart = search(&entries, "restart");
        let power_kinds_restart: HashSet<PowerCommandKind> = result_restart
            .iter()
            .filter_map(|e| match e {
                Entry::PowerCommand(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert!(
            power_kinds_restart.contains(&PowerCommandKind::Restart),
            "query \"restart\" should match PowerCommand::Restart; got {power_kinds_restart:?}"
        );
        assert!(
            !power_kinds_restart.contains(&PowerCommandKind::LockSession),
            "query \"restart\" should NOT match PowerCommand::LockSession; got {power_kinds_restart:?}"
        );
        assert!(
            !power_kinds_restart.contains(&PowerCommandKind::Suspend),
            "query \"restart\" should NOT match PowerCommand::Suspend; got {power_kinds_restart:?}"
        );
        assert!(
            !power_kinds_restart.contains(&PowerCommandKind::Shutdown),
            "query \"restart\" should NOT match PowerCommand::Shutdown; got {power_kinds_restart:?}"
        );
    }
}
