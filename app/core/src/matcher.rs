use crate::Entry;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Build the searchable text for an entry. Exhaustive on `Entry` so adding a
/// variant is a compile error until this is updated.
fn haystack(entry: &Entry) -> String {
    match entry {
        Entry::Application(app) => format!("{} {}", app.name, app.desktop_id),
    }
}

/// Fuzzy-search `entries` by `query`. An empty or whitespace-only query is a
/// passthrough that preserves input order. Otherwise the query is tokenized on
/// whitespace, every token must match the entry's haystack (intersection
/// semantics), and per-token scores are summed. Results are sorted by score
/// descending, with ascending name as the tiebreaker.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    if query.trim().is_empty() {
        return entries.iter().collect();
    }

    let tokens: Vec<&str> = query.split_whitespace().collect();
    let matcher = SkimMatcherV2::default().ignore_case();

    let mut scored: Vec<(&Entry, i64)> = Vec::new();
    for entry in entries {
        let hay = haystack(entry);
        let mut total: i64 = 0;
        let mut all_matched = true;
        for token in &tokens {
            match matcher.fuzzy_match(&hay, token) {
                Some(score) => total += score,
                None => {
                    all_matched = false;
                    break;
                }
            }
        }
        if all_matched {
            scored.push((entry, total));
        }
    }

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));
    scored.into_iter().map(|(e, _)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Application, Entry};
    use std::collections::HashSet;

    /// Test helper: build an `Entry::Application` with the given name/desktop_id
    /// and no icon. Kept terse so fixtures stay readable.
    fn app(name: &str, desktop_id: &str) -> Entry {
        Entry::Application(Application {
            name: name.into(),
            desktop_id: desktop_id.into(),
            icon: None,
        })
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
        assert_eq!(
            result[0].name(),
            "Chrome",
            "query \"google\" should match the Chrome entry (com.google.Chrome.desktop); got {:?}",
            result[0].name()
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
    fn score_sort_descending_with_name_tiebreaker() {
        // Both entries share the suffix "z.desktop" in their haystacks; the
        // query "z.desktop" matches that suffix identically in both. The
        // haystack prefix differs only in the name ("Bravo " vs "Alpha "),
        // which has no effect on the matched substring's score. With equal
        // scores the documented tiebreaker is ascending name, so "Alpha"
        // must appear before "Bravo".
        //
        // If a future fuzzy-matcher release weights haystack prefix length
        // and breaks the tie, this test would fail loudly and we would
        // adjust the fixture; per the plan we prefer a stable-ish assertion
        // over a flakier one.
        let entries = vec![app("Bravo", "z.desktop"), app("Alpha", "z.desktop")];

        let result = search(&entries, "z.desktop");

        assert_eq!(
            result.len(),
            2,
            "both entries should match \"z.desktop\"; got names {:?}",
            names(&result)
        );

        let result_names = names(&result);
        let alpha_pos = result_names
            .iter()
            .position(|n| *n == "Alpha")
            .expect("\"Alpha\" should be present in results");
        let bravo_pos = result_names
            .iter()
            .position(|n| *n == "Bravo")
            .expect("\"Bravo\" should be present in results");

        assert!(
            alpha_pos < bravo_pos,
            "with tied scores, alphabetical name tiebreaker should place \"Alpha\" before \"Bravo\"; got {:?}",
            result_names
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
}
