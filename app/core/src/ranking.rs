//! Centralized entry ranking for both platforms.
//!
//! NOTE FOR THE CODER: this file currently contains ONLY the test module. The
//! real implementations of `rank` and `mru_rank_map` are to be added ABOVE
//! this `#[cfg(test)] mod tests` block by the coder (per the architect's
//! plan). The tests below are written against the planned signatures:
//!
//!   pub fn rank(
//!       entries: &[Entry],
//!       query: &str,
//!       mru: &std::collections::HashMap<EntryRef, usize>,
//!   ) -> Vec<usize>
//!
//!   pub fn mru_rank_map(index: &[EntryRef]) -> std::collections::HashMap<EntryRef, usize>
//!
//! Do NOT delete or move the test module; merge the implementations above it.

use std::collections::HashMap;

use fuzzy_matcher::skim::SkimMatcherV2;

use crate::matcher;
use crate::{Entry, EntryRef};

/// Build a recency-rank map from an MRU index slice in most-recent-first
/// order: the first element gets rank 0 (most recent), the second rank 1, and
/// so on. The input is the `MruStore::read_all()` ordering. This is the shared
/// helper both platforms use to construct the `mru` map they pass to `rank`
/// (the GNOME `populate_list` path and the FFI `apply_mru` path), so the two
/// frontends agree on what "rank" means.
pub fn mru_rank_map(index: &[EntryRef]) -> HashMap<EntryRef, usize> {
    index
        .iter()
        .cloned()
        .enumerate()
        .map(|(rank, r)| (r, rank))
        .collect()
}

/// Rank `entries` against `query`, returning the surviving entries' indices
/// (into `entries`) in display order.
///
/// Filtering uses the same intersection (every whitespace token must match)
/// semantics as `matcher::search` via `matcher::score`; entries that fail any
/// token are dropped. Survivors are split into two tiers:
///
///   - Tier 1 — entries present in `mru` (the user has launched them). Sorted
///     by `(prefix_bucket, recency_rank)`: a `prefix_bucket` of 0 (the query
///     is a word-prefix match per `matcher::is_prefix_match`) sorts ahead of
///     bucket 1 (matched only fuzzily). Within a bucket, lower recency rank
///     (more recent) comes first. The prefix sub-bucket reorders ONLY within
///     Tier 1 — a never-used entry can never be promoted above an MRU one,
///     even if the never-used entry is a stronger prefix match.
///   - Tier 2 — entries absent from `mru` (never used). Sorted by descending
///     fuzzy score, so the best textual match in the never-used set comes
///     first.
///
/// Tier 1 then Tier 2 are concatenated. Both sorts are stable
/// (`sort_by_key`), so ties keep input order.
///
/// Empty / whitespace-only query is a passthrough: `tokens` is empty, so
/// `matcher::score` returns `Some(0)` for every entry (all included) and
/// `matcher::is_prefix_match` is vacuously true (every Tier 1 entry lands in
/// prefix_bucket 0, leaving them in pure recency order); Tier 2 entries all
/// score 0 and keep input order. Net result: MRU entries by recency, then
/// never-used entries in input order.
///
/// Both `matcher::score` and `matcher::is_prefix_match` are `pub(crate)` and
/// live in the same crate, so this is an in-crate call with no FFI surface.
pub fn rank(entries: &[Entry], query: &str, mru: &HashMap<EntryRef, usize>) -> Vec<usize> {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    let matcher = SkimMatcherV2::default().ignore_case();

    // (prefix_bucket, recency_rank, index)
    let mut tier1: Vec<(u8, usize, usize)> = Vec::new();
    // (score, index)
    let mut tier2: Vec<(i64, usize)> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let Some(score) = matcher::score(entry, &tokens, &matcher) else {
            continue;
        };
        match mru.get(&entry.reference()) {
            Some(&rank) => {
                let bucket = if matcher::is_prefix_match(entry, &tokens) {
                    0
                } else {
                    1
                };
                tier1.push((bucket, rank, i));
            }
            None => tier2.push((score, i)),
        }
    }

    // Stable: ties (same bucket + rank) keep input order.
    tier1.sort_by_key(|(bucket, rank, _)| (*bucket, *rank));
    // Stable: descending score, ties keep input order.
    tier2.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

    tier1
        .into_iter()
        .map(|(_, _, i)| i)
        .chain(tier2.into_iter().map(|(_, i)| i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Application, Entry, EntryRef};
    use std::collections::HashMap;

    /// Test helper: build an `Entry::Application` with the given name/desktop_id
    /// and no icon. Mirrors the `app(...)` helper in `matcher.rs` so fixtures
    /// stay readable and consistent across the two test modules.
    fn app(name: &str, desktop_id: &str) -> Entry {
        Entry::Application(Application {
            name: name.into(),
            desktop_id: desktop_id.into(),
            icon: None,
            recent_window_id: None,
            is_running: false,
        })
    }

    /// Collect the names at the given `indices` (output of `rank`) into a
    /// `Vec<&str>` for order-based assertions.
    fn names<'a>(entries: &'a [Entry], indices: &[usize]) -> Vec<&'a str> {
        indices.iter().map(|&i| entries[i].name()).collect()
    }

    /// Build an MRU recency-rank map directly from a slice of desktop ids in
    /// most-recent-first order: ids[0] gets rank 0 (most recent), ids[1] rank
    /// 1, and so on. Kept independent of `mru_rank_map` so the `rank` tests do
    /// not depend on that function's behavior (which has its own dedicated
    /// test below).
    fn mru_for(ids: &[&str]) -> HashMap<EntryRef, usize> {
        ids.iter()
            .enumerate()
            .map(|(rank, id)| (EntryRef::Application((*id).into()), rank))
            .collect()
    }

    #[test]
    fn prefix_reorders_within_mru_tier() {
        // All three entries are in the MRU tier (recency Lock=0, Chrome=1,
        // Calc=2) and all three contain 'c', so all pass the "c" filter. The
        // prefix sub-bucket reorders WITHIN the MRU tier: Chrome ("c" prefixes
        // "Chrome") and Calc ("c" prefixes "Calc") are prefix matches (bucket
        // 0), ordered by recency (Chrome before Calc); Lock is a non-prefix
        // match (bucket 1), so it falls to the back despite being most recent.
        let entries = vec![
            app("Lock", "lock.desktop"),
            app("Google Chrome", "com.google.Chrome"),
            app("Calc", "calc.desktop"),
        ];
        let mru = mru_for(&["lock.desktop", "com.google.Chrome", "calc.desktop"]);

        let ranked = rank(&entries, "c", &mru);
        assert_eq!(
            names(&entries, &ranked),
            vec!["Google Chrome", "Calc", "Lock"],
            "prefix matches (Chrome, Calc) should sort ahead of the non-prefix MRU match (Lock); got {:?}",
            names(&entries, &ranked)
        );
    }

    #[test]
    fn mru_known_beats_never_used_prefix_match() {
        // Lock is in the MRU tier (rank 0) but is only a non-prefix match for
        // "c". Calculator is never-used (no MRU rank) and IS a prefix match.
        // The locked decision: a known (MRU) entry always outranks a
        // never-used one, even when the never-used one is a prefix match. So
        // Lock stays ahead of Calculator.
        let entries = vec![
            app("Lock", "lock.desktop"),
            app("Calculator", "calc.desktop"),
        ];
        let mru = mru_for(&["lock.desktop"]);

        let ranked = rank(&entries, "c", &mru);
        assert_eq!(
            names(&entries, &ranked),
            vec!["Lock", "Calculator"],
            "a known (MRU) entry must outrank a never-used prefix match; got {:?}",
            names(&entries, &ranked)
        );
    }

    #[test]
    fn never_used_tier_ordered_by_descending_score() {
        // Neither entry is in the MRU map, so both fall to the never-used
        // tier, which is ordered by descending fuzzy score. For query "code",
        // "Code" matches at the very start of its haystack (word-start,
        // contiguous) and scores higher than "Xcode", where "code" is a
        // contiguous substring but not at a word boundary. Both clearly pass
        // the `matcher::score` filter (each stripped haystack contains the
        // substring "code"), so this exercises descending-score ordering
        // within the never-used tier without depending on any vendor-prefix
        // behavior.
        let entries = vec![app("Code", "code.desktop"), app("Xcode", "xcode.desktop")];
        let mru: HashMap<EntryRef, usize> = HashMap::new();

        let ranked = rank(&entries, "code", &mru);
        assert_eq!(
            names(&entries, &ranked),
            vec!["Code", "Xcode"],
            "never-used tier should be ordered by descending score; got {:?}",
            names(&entries, &ranked)
        );
    }

    #[test]
    fn empty_query_mru_order_then_input_order() {
        // Empty query is a passthrough: all entries are included. MRU-ranked
        // entries come first in recency order (Charlie=0, Alpha=1), then the
        // never-used entries in input order (Bravo, then Delta). A
        // whitespace-only query behaves identically.
        let entries = vec![
            app("Alpha", "a.desktop"),
            app("Bravo", "b.desktop"),
            app("Charlie", "c.desktop"),
            app("Delta", "d.desktop"),
        ];
        let mru = mru_for(&["c.desktop", "a.desktop"]);

        let ranked = rank(&entries, "", &mru);
        assert_eq!(
            names(&entries, &ranked),
            vec!["Charlie", "Alpha", "Bravo", "Delta"],
            "empty query should list MRU entries by recency then never-used in input order; got {:?}",
            names(&entries, &ranked)
        );

        let ranked_ws = rank(&entries, "   ", &mru);
        assert_eq!(
            names(&entries, &ranked_ws),
            vec!["Charlie", "Alpha", "Bravo", "Delta"],
            "whitespace-only query should behave identically to the empty query; got {:?}",
            names(&entries, &ranked_ws)
        );
    }

    #[test]
    fn multi_token_requires_all_word_prefixes() {
        // Query "go ch": "Google Chrome" satisfies both word-prefixes
        // (go->Google, ch->Chrome). "Gnome Calc" is FILTERED OUT entirely
        // because no word starts with "ch" (and the fuzzy score also fails
        // the "ch" token: there is no 'h'), so it does not appear at all.
        let entries = vec![
            app("Google Chrome", "com.google.Chrome"),
            app("Gnome Calc", "gc.desktop"),
        ];
        let mru = mru_for(&["gc.desktop", "com.google.Chrome"]);

        let ranked = rank(&entries, "go ch", &mru);
        assert_eq!(
            names(&entries, &ranked),
            vec!["Google Chrome"],
            "\"go ch\" should match only Google Chrome (Gnome Calc has no 'h' to satisfy \"ch\"); got {:?}",
            names(&entries, &ranked)
        );

        // Query "gc": both entries fuzzy-match (scattered g..c), and neither
        // is a word-prefix match ("gc" prefixes no single word of either
        // name). Both land in the non-prefix sub-bucket of the MRU tier, so
        // they sort by recency: Gnome Calc (rank 0) before Google Chrome
        // (rank 1).
        let ranked_gc = rank(&entries, "gc", &mru);
        assert_eq!(
            names(&entries, &ranked_gc),
            vec!["Gnome Calc", "Google Chrome"],
            "\"gc\" should keep both in the non-prefix bucket, ordered by recency; got {:?}",
            names(&entries, &ranked_gc)
        );
    }

    #[test]
    fn case_insensitivity() {
        // Reuse fixture #1: an uppercase query "C" must yield the exact same
        // ordering as the lowercase "c" — case must not affect filtering,
        // prefix bucketing, or ordering.
        let entries = vec![
            app("Lock", "lock.desktop"),
            app("Google Chrome", "com.google.Chrome"),
            app("Calc", "calc.desktop"),
        ];
        let mru = mru_for(&["lock.desktop", "com.google.Chrome", "calc.desktop"]);

        let ranked = rank(&entries, "C", &mru);
        assert_eq!(
            names(&entries, &ranked),
            vec!["Google Chrome", "Calc", "Lock"],
            "uppercase \"C\" should produce the same order as lowercase \"c\"; got {:?}",
            names(&entries, &ranked)
        );
    }

    #[test]
    fn mru_rank_map_assigns_recency_ranks() {
        // mru_rank_map maps an index slice (most-recent-first) to ranks: the
        // first element gets rank 0, the second rank 1, etc. Keys not present
        // in the slice resolve to None.
        let index = vec![
            EntryRef::Application("a.desktop".into()),
            EntryRef::Application("b.desktop".into()),
        ];

        let map = mru_rank_map(&index);

        assert_eq!(
            map.get(&EntryRef::Application("a.desktop".into())),
            Some(&0),
            "the most-recent entry (a.desktop) should be rank 0; got {:?}",
            map.get(&EntryRef::Application("a.desktop".into()))
        );
        assert_eq!(
            map.get(&EntryRef::Application("b.desktop".into())),
            Some(&1),
            "the second entry (b.desktop) should be rank 1; got {:?}",
            map.get(&EntryRef::Application("b.desktop".into()))
        );
        assert_eq!(
            map.get(&EntryRef::Application("absent.desktop".into())),
            None,
            "an entry not in the index should have no rank; got {:?}",
            map.get(&EntryRef::Application("absent.desktop".into()))
        );
    }
}
