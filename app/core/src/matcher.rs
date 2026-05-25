use crate::Entry;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Strip a leading reverse-DNS top-level segment (e.g. `com.`, `org.`) from a
/// bundle/desktop ID before it enters the matcher haystack.
///
/// Every macOS bundle identifier starts with the same handful of TLD-style
/// segments — `com.apple.*`, `com.google.*`, `org.mozilla.*` — so leaving
/// them in the haystack turns short queries into noise: a one-character `m`
/// fuzzy-matches every `com.apple.*` ID via the `m` in `com.`. Stripping the
/// TLD segment removes that shared-letter floor while keeping the rest of
/// the identifier searchable. `"google"` still matches
/// `"com.google.Chrome.desktop"` (via the remaining `google.Chrome.desktop`),
/// and `"acrobat"` still matches `"com.adobe.Acrobat"`.
///
/// Only the first segment is stripped — stripping the vendor too would
/// regress the deliberate "find apps by vendor" path covered by
/// `single_token_matches_desktop_id` below.
fn strip_reverse_dns_tld(id: &str) -> &str {
    const TLDS: &[&str] = &["com.", "org.", "net.", "io."];
    for tld in TLDS {
        if let Some(rest) = id.strip_prefix(tld) {
            return rest;
        }
    }
    id
}

/// Build the searchable text for an entry. Exhaustive on `Entry` so adding a
/// variant is a compile error until this is updated.
fn haystack(entry: &Entry) -> String {
    match entry {
        Entry::Application(app) => {
            format!("{} {}", app.name, strip_reverse_dns_tld(&app.desktop_id))
        }
        Entry::Window(w) => match &w.app_name {
            Some(app) => format!("{} {}", w.title, app),
            None => w.title.clone(),
        },
        Entry::Workspace(w) => w.name.clone(),
        Entry::Command(c) => c.kind.display_name().to_string(),
        Entry::PowerCommand(c) => c.kind.display_name().to_string(),
    }
}

/// Score a single `entry` against every whitespace-separated token in
/// `tokens` (intersection / case-insensitive semantics). Returns `None` if
/// any token fails to fuzzy-match the haystack; otherwise returns the sum
/// of per-token scores from `SkimMatcherV2::fuzzy_match`.
///
/// Higher scores correspond to better matches — substring hits beat
/// scattered-subsequence hits, prefix hits beat infix hits. The FFI layer
/// (`lofi_entries_set_query`) and `search` below both use this score for
/// ranking: a query like `"Code"` puts `"Visual Studio Code"` above
/// `"Acrobat"` because the substring hit scores far higher than the
/// scattered `c…o…d…e` pulled out of `"com.adobe.Acrobat"`.
///
/// `tokens` is expected to be already tokenized (typically via
/// `query.split_whitespace().collect::<Vec<_>>()`); `matcher` is the
/// case-insensitive skim matcher. Both are passed in so the caller can
/// reuse them across many entries.
pub(crate) fn score(entry: &Entry, tokens: &[&str], matcher: &SkimMatcherV2) -> Option<i64> {
    let hay = haystack(entry);
    let mut total: i64 = 0;
    for token in tokens {
        let s = matcher.fuzzy_match(&hay, token)?;
        total = total.saturating_add(s);
    }
    Some(total)
}

/// Fuzzy-filter `entries` by `query`. An empty or whitespace-only query is
/// a passthrough that returns every entry in input order. Otherwise the
/// query is tokenized on whitespace, every token must match the entry's
/// haystack (intersection semantics), and matching entries are returned
/// sorted by descending score — best matches first.
///
/// Why score-based: pure fuzzy subsequence matching is too permissive on
/// its own (`"Code"` matches `"Acrobat com.adobe.Acrobat"` via scattered
/// letters); ranking by score is what makes `"Visual Studio Code"` show
/// up first instead of buried under noise. The FFI layer overlays MRU
/// recency on top — see `EntryList::recompute_filter`.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    if query.trim().is_empty() {
        return entries.iter().collect();
    }

    let tokens: Vec<&str> = query.split_whitespace().collect();
    let matcher = SkimMatcherV2::default().ignore_case();

    let mut scored: Vec<(i64, &Entry)> = entries
        .iter()
        .filter_map(|entry| score(entry, &tokens, &matcher).map(|s| (s, entry)))
        .collect();
    // Descending by score; ties retain input order (sort_by is stable).
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, e)| e).collect()
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
        // "Files" intentionally has a reverse-DNS desktop_id. Pre-TLD-strip,
        // "FIRE" used to fuzzy-match it via the `r` in `org.` and the `e` in
        // `gnome` — exactly the noise the strip is designed to remove. The
        // negative assertion below pins that this no longer happens.
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
            !name_set.contains("Files"),
            "query \"FIRE\" should NOT match \"Files\" — the previous match relied on the `r` and `e` inside the stripped `org.gnome.` prefix noise; got names {:?}",
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
    fn single_char_does_not_match_via_reverse_dns_tld_prefix() {
        // Regression: every macOS bundle id begins with `com.` (or `org.`,
        // `net.`, `io.`), so a one-character query like `"m"` would otherwise
        // fuzzy-match every Apple app via the `m` in `com.`. The haystack
        // strips the leading TLD segment so single-character matches must
        // come from the name or the post-TLD portion of the id.
        let entries = vec![
            app("Calculator", "com.apple.Calculator"),
            app("Safari", "com.apple.Safari"),
            app("Maps", "com.apple.Maps"),
        ];

        let result = search(&entries, "m");
        let name_set: HashSet<&str> = result.iter().map(|e| e.name()).collect();

        assert!(
            name_set.contains("Maps"),
            "query \"m\" should match \"Maps\" (the name and the post-TLD id both contain `m`); got names {:?}",
            name_set
        );
        assert!(
            !name_set.contains("Calculator"),
            "query \"m\" should NOT match \"Calculator\" — the only `m` is in the stripped `com.` prefix; got names {:?}",
            name_set
        );
        assert!(
            !name_set.contains("Safari"),
            "query \"m\" should NOT match \"Safari\" — the only `m` is in the stripped `com.` prefix; got names {:?}",
            name_set
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
            power(PowerCommandKind::Logout),
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
        assert!(
            !power_kinds_suspend.contains(&PowerCommandKind::Logout),
            "query \"suspend\" should NOT match PowerCommand::Logout; got {power_kinds_suspend:?}"
        );

        // Query "log" should match both LockSession and Logout among
        // PowerCommands ("log" is a subsequence of "Log Out", and the
        // fuzzy matcher accepts "lo...c...k" against "Lock" → "ock"
        // sequence). The point of this case is the positive Logout
        // match — the LockSession side-match is incidental.
        let result_log = search(&entries, "log");
        let power_kinds_log: HashSet<PowerCommandKind> = result_log
            .iter()
            .filter_map(|e| match e {
                Entry::PowerCommand(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert!(
            power_kinds_log.contains(&PowerCommandKind::Logout),
            "query \"log\" should match PowerCommand::Logout; got {power_kinds_log:?}"
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
        assert!(
            !power_kinds_restart.contains(&PowerCommandKind::Logout),
            "query \"restart\" should NOT match PowerCommand::Logout; got {power_kinds_restart:?}"
        );
    }
}
