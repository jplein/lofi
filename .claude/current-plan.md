# Plan: Prefix-match ranking signal + consolidate ranking into lofi_core

## Status: COMPLETE ✅

## Problem statement

Entry ranking uses recency (MRU) as the only ordering signal within the MRU tier, and
ranking logic is **duplicated**: macOS FFI `recompute_filter` (reorders the `entries` vec
in `apply_mru`, tracks `mru_count`) and GNOME `populate_list` (search + manual
`mru_position` sort). Goal: add a prefix-match signal (reordering ONLY within the MRU tier)
AND consolidate all ranking into a single `lofi_core::rank()` both platforms call.

### Locked decisions
- Prefix reorders WITHIN the MRU tier only; a recently-used app still outranks a never-used one.
- Prefix = query token is a case-insensitive prefix of ANY whitespace-separated word in the
  entry's haystack. Multi-token = prefix match only if EVERY token is a word-prefix.
- Empty query = passthrough: all entries in MRU order (recent first), then never-used in input order.

### Target architecture
`Platform collects entries -> lofi_core::rank(entries, query, &mru_rank_map) -> platform displays`

---

## Architect's plan

### Files to create

**`app/core/src/ranking.rs`** (new) — `mod ranking;` in lib.rs, `pub use ranking::{rank, mru_rank_map};`.
Imports: `std::collections::HashMap`, `fuzzy_matcher::skim::SkimMatcherV2`, `crate::matcher`, `crate::{Entry, EntryRef}`.

- `pub fn mru_rank_map(index: &[EntryRef]) -> HashMap<EntryRef, usize>`:
  `index.iter().cloned().enumerate().map(|(rank, r)| (r, rank)).collect()`. rank 0 = most recent.
  `EntryRef` already derives `Clone + Hash + Eq` (lib.rs ~line 421) — NO derive change needed.
- `pub fn rank(entries: &[Entry], query: &str, mru: &HashMap<EntryRef, usize>) -> Vec<usize>`:
  1. `let tokens: Vec<&str> = query.split_whitespace().collect();`
  2. ONE matcher: `SkimMatcherV2::default().ignore_case()`.
  3. For each `(i, entry)`: `let Some(score) = matcher::score(entry, &tokens, &matcher) else { continue };`
     (empty tokens => Some(0) => all included → passthrough).
  4. Partition: Tier 1 if `mru.get(&entry.reference())` is Some(rank) → record `(prefix_bucket, rank, i)`
     with `prefix_bucket = if matcher::is_prefix_match(entry, &tokens) {0} else {1}`;
     Tier 2 (None) → record `(score, i)`.
  5. Tier 1 STABLE sort by `(prefix_bucket, rank)`.
  6. Tier 2 STABLE sort by `Reverse(score)`.
  7. Concatenate Tier1 then Tier2 indices.
  Doc: filtering, two tiers, prefix sub-bucket within Tier 1 only (never above never-used boundary),
  empty-query passthrough explanation.

### Files to modify

**`app/core/src/matcher.rs`** — add after `score`:
`pub(crate) fn is_prefix_match(entry: &Entry, tokens: &[&str]) -> bool`. Reuse SAME `haystack()`.
Lowercase haystack once, split to words; `tokens.iter().all(|t| { let tl = t.to_lowercase();
words.iter().any(|w| w.starts_with(&tl)) })`. Empty tokens => vacuously true. Do NOT use
`eq_ignore_ascii_case`. Doc-comment the word-prefix + multi-token AND rule. `search()` unchanged.

**`app/core/src/lib.rs`** — add `mod ranking;` and `pub use ranking::{rank, mru_rank_map};`
near `pub use matcher::search;`. Keep `search` re-export.

**`app/core/src/ffi/entries.rs`**
- Struct: replace `pub(super) mru_count: usize` with `pub(super) mru_ranks: HashMap<EntryRef, usize>`.
  Update field doc (recency-rank map; apply_mru no longer reorders the vec).
- `new()`: `mru_ranks: HashMap::new()`. `clear()`: `self.mru_ranks = HashMap::new();`.
- `recompute_filter()`: body becomes `self.filter = Some(crate::ranking::rank(&self.entries,
  &self.query, &self.mru_ranks));`. Remove now-unused `SkimMatcherV2` + `crate::matcher` imports
  (keep `use std::collections::HashMap;`). `filter` stays `Option` but is `Some` after every
  recompute; `None` only on fresh/cleared list. Rewrite the ordering-rule doc comment to point at
  `ranking::rank`.
- `apply_mru()`: keep null/read-failure guards (return false, list untouched). New body:
  read_all → `list_ref.mru_ranks = crate::ranking::mru_rank_map(&recent);` → `clear_caches()` →
  `recompute_filter()` → true. Remove the `paired` drain/sort/`mru_count` block. Rewrite doc comment.
- Module-level borrow-contract docs: apply_mru rebuilds the rank map and recomputes (vec NOT
  reordered); filter is `None` only on fresh/cleared list, else `Some`. Cache-keying note unchanged.
- FFI signatures byte-identical → NO cbindgen header change, NO Swift change (field is inside opaque struct).

**`app/gnome/src/ui.rs`**
- Line 10 import: drop `search` → `use lofi_core::{Entry, EntryKind, EntryRef, MruStore};`.
- `populate_list`: replace the whole `new_visible` computation (empty-query branch + search call +
  ptr::eq loop + manual sort) with:
  ```rust
  let new_visible: Vec<usize> = {
      let s = state.borrow();
      lofi_core::rank(&s.entries, query, &s.mru_position)
  };
  ```
  Keep `mru_position` (already the rank map). "No matches" branch + visible swap unchanged.
  Rewrite the `populate_list` doc comment.

**`app/gnome/src/main.rs`** (recommended DRY, optional): build `mru_position` via
`lofi_core::mru_rank_map(&mru_index)` and pass the map to `ui::build` (changing its signature to
take `HashMap<EntryRef, usize>` and dropping the inline `.enumerate()` map in `build`). Either do
the DRY refactor consistently OR leave `ui::build` taking `Vec<EntryRef>` and change nothing in
main.rs. Coder picks one.

### Test plan

**`matcher.rs` (add to existing tests, reuse `app(name, desktop_id)`):**
- word-prefix yes (`["c"]`/`["goo"]` vs "Google Chrome"), mid-word no (`["hrome"]`), scattered no
  (`["gc"]`), empty tokens true, case-insensitive (`["C"]`==`["c"]`), multi-token
  (`["go","ch"]` true, `["go","xy"]` false).

**`ranking.rs` (new in-module tests):** local `app()` helper + `names(entries, indices)` helper;
build `mru` maps with `EntryRef::Application(id.into())` keys.
1. `prefix_reorders_within_mru_tier` — entries `app("Lock","lock.desktop")`,
   `app("Google Chrome","com.google.Chrome")`, `app("Calc","calc.desktop")`; mru ranks
   Lock=0,Chrome=1,Calc=2; query "c" → `["Google Chrome","Calc","Lock"]` (Chrome+Calc bucket 0 by
   recency, Lock bucket 1 last). (All three contain 'c' so all pass the filter — fixture chosen for this.)
2. `mru_known_beats_never_used_prefix_match` — `app("Lock","lock.desktop")`,
   `app("Calculator","calc.desktop")`; mru only Lock=0; query "c" → `["Lock","Calculator"]`.
3. `never_used_tier_ordered_by_descending_score` — `app("Acrobat","com.adobe.Acrobat")`,
   `app("Visual Studio Code","com.microsoft.VSCode")`; empty mru; query "code" →
   `["Visual Studio Code","Acrobat"]`.
4. `empty_query_mru_order_then_input_order` — Alpha/Bravo/Charlie/Delta; mru Charlie=0,Alpha=1;
   query "" → `["Charlie","Alpha","Bravo","Delta"]`; also assert "   " identical.
5. `multi_token_requires_all_word_prefixes` — `app("Google Chrome","com.google.Chrome")`,
   `app("Gnome Calc","gc.desktop")`; mru Chrome=1,GnomeCalc=0; query "go ch" → `["Google Chrome"]`
   (Gnome Calc filtered out — no 'h'); query "gc" → `["Gnome Calc","Google Chrome"]` (both bucket 1,
   recency order).
6. `case_insensitivity` — fixture #1, query "C" identical order to "c".

**`app/core/tests/ffi.rs` (one integration test, reuse existing helpers):**
`apply_mru_then_query_ranks_prefix_within_mru` — push `Lock/lock.desktop`,
`Google Chrome/com.google.Chrome`, `Calc/calc.desktop`; bump in order Calc, Chrome, Lock so
recency Lock(0)>Chrome(1)>Calc(2) (follow existing bump pattern; add `mru.rs`-style sleeps only if
flaky); `apply_mru`; `set_query("c")`; assert `name_at` order `["Google Chrome","Calc","Lock"]` and
`len==3`.
**Existing FFI tests: NONE need adjustment** — all read through filtered accessors (`name_at` →
`resolved` → filter), and `rank` reproduces the prior MRU-then-score ordering. Verified:
`mru_bump_then_apply_promotes_entry`, `apply_mru_with_empty_store_preserves_input_order`,
`mru_persists_across_open`, `apply_mru_invalidates_caches`, `apply_mru_with_query_active_keeps_filter`,
null/OOB tests all still pass.

### Docs (READMEs source of truth)
- `app/core/README.md`: document centralized `rank` (filtering, two tiers, prefix definition,
  multi-token rule, empty-query passthrough); ranking is core's responsibility; note new `ranking.rs`
  and `mru_count`→`mru_ranks` if internals are described.
- `app/gnome/README.md` + `app/macos/README.md`: ranking now comes from core `rank()`; remove/adjust
  platform-side MRU-sort + "apply_mru reorders the vec" descriptions.

### Implementation order (do 1–3 together to avoid transient dead-code lint)
1. matcher.rs `is_prefix_match` (+ tests). 2. ranking.rs (+ tests). 3. lib.rs wiring.
4. ffi/entries.rs (struct/new/clear/recompute_filter/apply_mru/docs; remove unused imports).
5. tests/ffi.rs integration test. 6. gnome ui.rs (+ optional main.rs). 7. READMEs.

### Lint / deps (Rust — Go notes in brief are N/A)
- No new crates; no manifest edits.
- Remove unused `SkimMatcherV2`/`crate::matcher` imports from ffi/entries.rs after delegation.
- Keep `use std::collections::HashMap;` in ffi/entries.rs (struct field uses it).
- `.to_lowercase()` in is_prefix_match is fine; do NOT switch to `eq_ignore_ascii_case`.
- Run cargo fmt; CI = `cargo fmt --check`, `cargo clippy`, `cargo test` (core + gnome).

### Risks / gotchas
- apply_mru no longer reorders the vec — only consumer of vec order was recompute_filter (now reads
  mru_ranks). Caches keyed on underlying index are MORE robust (vec doesn't move). grep to confirm
  no other reader of `mru_count`.
- `EntryRef: Clone+Hash+Eq` already present — don't re-derive.
- Empty-query passthrough relies on `score`=Some(0) AND `is_prefix_match`=true for empty tokens (test #4 guards).
- empty query now → `filter = Some(all-indices)` in MRU order (was `None`); net behavior preserved
  because old apply_mru pre-sorted the vec.
- FFI header stays byte-identical (private field in opaque struct).
- GNOME populate_list: read `&s.entries` + `&s.mru_position` in one borrow scope before the borrow_mut swap.
- Stable sorts required for tiebreaks (`sort_by_key` is stable).

## Workflow status
- [x] Architect plan
- [x] Test-writer pass — matcher.rs (6 is_prefix_match tests), ranking.rs (7 tests), ffi.rs (1
  integration test). Orchestrator routed one fixture fix back: `never_used_tier_ordered_by_descending_score`
  changed from Acrobat/VSCode (relied on now-stripped vendor prefix) to Code/Xcode (query "code" →
  ["Code","Xcode"]).
- [x] Coder pass — implemented is_prefix_match (matcher.rs), rank + mru_rank_map (ranking.rs),
  lib.rs wiring, ffi/entries.rs (mru_count → mru_ranks, recompute_filter/apply_mru delegate to
  ranking, removed unused imports, updated borrow-contract docs), gnome ui.rs (delegates to
  lofi_core::rank, dropped search import). main.rs left unchanged (no DRY refactor). **All checks
  green: cargo build + build --features ffi PASS; clippy (both) warning-free; fmt --check PASS;
  145 Rust tests pass (85 lib + 53 ffi + 4 mru + 3 gnome), 0 failures.** No FFI signature/header change.
- [x] Reviewer pass — **APPROVED**, no BLOCKER/MAJOR. Verified rank ordering (prefix only within
  MRU tier; never-used prefix can't outrank MRU), is_prefix_match semantics, empty-query passthrough,
  apply_mru no-reorder + filter always Some, no leftover mru_count, unused imports removed, gnome
  borrow scoping, EntryRef bounds, test fixtures valid, cross-platform parity. MINOR: app/core/README.md
  stale (mru_count / "reorders the vec") → technical-writer. NIT: two stale comments in tests/ffi.rs
  (:274-277, :432) describe old mechanism but assertions correct — optional cleanup.
- [x] Technical-writer pass — updated app/core/README.md (new ranking::rank/mru_rank_map section,
  removed mru_count + vec-reorder descriptions, clarified search is the score-only primitive) and
  app/gnome/README.md (populate_list delegates to lofi_core::rank). app/macos/README.md needed no
  change (only refers to MRU generically). Also fixed the two NIT stale comments in tests/ffi.rs
  (comment-only). Final re-verify: 142 tests pass with --features ffi, clippy/fmt clean.
