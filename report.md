# LoFi review — core Rust (`app/core`) + macOS Swift (`app/macos`)

Scope: the platform-agnostic Rust core and the macOS Swift frontend. The GNOME
Rust crate (`app/gnome`) and the TypeScript extension (`extension/gnome`) are
out of scope (the extension is checked on Linux).

Review criteria: separation of concerns, non-idiomatic/deprecated patterns,
tests pass, linter checks pass.

---

## Gate status

| Gate | Result |
|---|---|
| Rust tests (`bazelisk test //app/...`) | ✅ pass — `ffi_test`, `mru_test` (newly wired), `rustfmt` |
| clippy (`//app/core:clippy`, warnings→errors) | ✅ clean |
| rustfmt (`//app/core:rustfmt`) | ✅ clean |
| Swift build (`//app/macos`) | ✅ compiles |
| Swift lint/format (`app/macos/check.sh`) | ✅ configured this session — swift-format; clean after one reformat pass |

Note: `bazelisk test //app/...` is the macOS path. On Linux the equivalent is
`cargo test` / `cargo clippy --all-targets` / `cargo fmt --check` via the
direnv + flake toolchain (Bazel is not installed on Linux).

### Lint/format wiring done this session

The Rust lint/format path was previously unwired on the Bazel (macOS) side.
Fixed:

- **`.bazelignore`** (new) — stops Bazel descending into `app/target/` (Cargo
  output; was racing with rust-analyzer temp dirs and erroring) and `.direnv/`
  (was producing duplicate `//.direnv/...` targets under `//...`).
- **`app/core/BUILD.bazel`** — added `:clippy` (`rust_clippy`, warnings→errors),
  `:rustfmt` (`rustfmt_test`), and `mru_test` (the previously orphaned
  `tests/mru.rs`, which was neither run nor lint/format-covered under Bazel).
- **`CLAUDE.md` + `app/core/README.md`** — documented per-platform check
  commands, with Bazel flagged macOS-only.

Verified: `bazelisk test //app/...` now runs compile + clippy + rustfmt + all
tests in one command, is green, and fails when a clippy lint is introduced.

---

## Core Rust (`app/core`)

### 🔴 The matcher's docs say "filter-only," but the code ranks by score — and the two platforms now order results differently

`matcher::search` (`src/matcher.rs:85-100`) computes a fuzzy score per entry and
sorts `sort_by_key(Reverse(score))`. The documentation still describes the older
filter-only design in several places:

- `app/core/README.md:182` — "`search` is **filter-only**… does not rank or
  score… the classic Raycast-style 'selection shifts mid-keystroke' is what
  filter-only + caller-sorted prevents."
- `app/core/README.md:233` — MRU is "the **sole sort key** for the displayed list."
- `app/core/README.md:318` — "the matcher's **filter-only semantics** then
  preserve the MRU order."
- `app/core/README.md:339` — `set_query` is "**exactly matching**
  `matcher::search`'s semantics."
- `src/matcher.rs:361` (test comment in `matching_entries_are_returned_regardless_of_order`)
  — "search() is filter-only now… no score-based ranking."

Per CLAUDE.md ("READMEs are the source of truth; a README/code disagreement is a
bug") this is a documentation bug. But the stale docs are a symptom of a deeper
divergence — the two platforms' ordering is no longer the same, and only one
path is documented correctly:

- **macOS** (`src/ffi/entries.rs::recompute_filter`, correctly documented at
  lines 177-189): *MRU-known entries keep recency order; only never-launched
  entries are sorted by score.* This is a deliberate two-tier "MRU wins, then
  score" policy.
- **GNOME** (`matcher::search`): *all* matches sort by score, so for any
  non-empty query the MRU ordering is overridden rather than preserved.
  README:318 and :339 claim the `set_query` (FFI) and `search` paths are
  equivalent; they are not.

Impact: not a crash, and score ranking is still deterministic, but it
contradicts the documented "MRU is the sole sort key" design, contradicts the
stated *Predictable* goal in the top-level README, and makes the same query rank
differently on the two platforms.

Recommendation: decide which ordering is intended, align both code paths, then
update the four doc spots + the test comment. The code looks deliberate, so the
most likely resolution is that the README/comments simply need to catch up to
the macOS two-tier behavior — but the GNOME `search` path should probably adopt
the same two-tier policy so the platforms match.

### Otherwise: clean, and a model for the rest of the repo

No other findings. Worth calling out as done well:

- Pure logic is fully isolated from platform code; `core` has no GTK / AppKit /
  D-Bus / windowing dependency.
- Exhaustive `match` on `Entry` and the `*Kind` enums (no `_` arms), so adding a
  variant is a compile error until every accessor/map is updated.
- `MruError` is a typed enum with `From`/`Display`/`source`; nothing panics;
  `read_all` tolerates corrupt rows (logs and skips) — degraded mode is "forget
  recents," never "crash."
- `compute_geometry` and `build_workspace_commands` are pure and exhaustively
  unit-tested (all kinds, boundary/sticky/empty cases).
- The `EntryRef` (persistence handle) vs `Entry` (runtime) split is well-reasoned,
  including the deliberate refusal to derive serde on `Entry`/`Application`.
- No deprecated patterns; no unused dependencies in `core`.

---

## macOS Swift (`app/macos`)

No blockers; the FFI boundary is handled with real discipline. Polish items:

### 🟠 MED

- **`app/macos/README.md:24`** — stale: "Swift only reads it back through
  `lofi_entries_len` / `lofi_entries_get_name`." The actual read surface is
  `lofi_entries_len` + **9** `lofi_entries_get_*` accessors (name, bundle_id,
  category, icon, window_id, is_running, command_id, command_geometry,
  power_command_id), each wrapped 1:1 in `RustBridge.swift` — 10 read functions,
  not 2. CLAUDE.md "bug"; generalize to "the `lofi_entries_get_*` accessors" and
  defer to `app/core/README.md` (the authoritative, current FFI list) rather
  than re-enumerating here. *Severity MED-leaning-LOW: contained to one narrative
  sentence; the canonical FFI doc in `app/core/README.md` is correct.*
  **Fixed this session.**

### 🟡 LOW

- **`AppDelegate.swift:296`** — `NSApp.activate(ignoringOtherApps: true)`.
  *Originally flagged MED as "deprecated since macOS 14" — that was wrong for the
  Xcode 26 SDK and is downgraded here.* The SDK marks it
  `API_DEPRECATED("...will be deprecated in a future release. Use NSApp.activate
  instead.", macos(10.0, API_TO_BE_DEPRECATED))` — `API_TO_BE_DEPRECATED` is a
  far-future sentinel, not a real version, so at the 15.0 deployment target there
  is **no compiler warning** (confirmed from the SDK header's deprecation
  annotation; a clean build of the module emits none). Apple actually
  walked the hard `deprecated: 14.0` annotation back to this advisory form because
  the cooperative-activation replacement (`NSApp.activate()`, no-arg, best-effort)
  is *not* a reliable substitute for forcing an `LSUIElement` accessory app
  forward on a global-hotkey summon — exactly what this call does before
  `panelController.show()` (`makeKeyAndOrderFront`). Recommendation: leave as-is;
  it's the correct API for guaranteed activation and there's no committed removal
  version. Optionally add a one-line comment noting `NSApp.activate` was
  considered and rejected as unreliable here. Revisit only if Apple assigns a
  concrete deprecation version.
- **`as!` force-casts** — originally filed as one item over `WindowControl.swift`
  (AX position/size) and `WindowDiscovery.swift` (`kCGWindowBounds`). They turned
  out to be two different cases:
  - `WindowDiscovery.swift` — the value is `Any` (from `[String: Any]`), so
    `as! CFDictionary` *is* a real checked cast that can trap. **Fixed this
    session** → `as? NSDictionary` + `as CFDictionary` (the loop already
    `continue`s on bad fields).
  - `WindowControl.swift` — **false positive.** The values are `CFTypeRef`, and a
    downcast to a CoreFoundation type (`AXValue`) never traps (Swift treats it as
    unconditional — `as?` even warns "will always succeed"). The real validation
    is `AXValueGetValue` returning false → the existing `guard` returns `nil`. No
    crash risk. Reverted to `as!` and added a comment explaining why it's safe, so
    it isn't re-flagged.
- **`Hotkey.swift`** — `InstallEventHandler` / `RegisterEventHotKey` `OSStatus`
  returns were discarded (silent dead hotkey on failure). **Fixed this session**:
  capture status, `NSLog` on non-`noErr`.
- **`PanelController.isVisible`** — dead code with a doc reference to the renamed
  `toggleOrSummon`. **Fixed this session**: removed.
- **`windowAux`** is captured by value at controller init while `commandTarget`
  is refreshed every summon (no `setWindowAux`). Harmless today because the window
  switcher is disabled (map always empty), latent if Window rows return.
  **Addressed this session**: added a comment flagging the asymmetry (chose not to
  add machinery for a disabled feature).

### Well done (Swift)

- The FFI bridge is disciplined: opaque handles stay `private`; every
  `lofi_entries_get_*` C string is copied into a Swift `String` at the accessor,
  so the borrow contract (valid only until the next mutating call) is honored
  everywhere; handles are freed exactly once in `deinit` (no double-free / leak).
- The top-left/bottom-left coordinate flip is isolated in one place
  (`WindowCommands.workAreaTopLeft`), never repeated in `WindowControl`.
- The private `_AXUIElementGetWindow` symbol is resolved via
  `dlsym(RTLD_DEFAULT)` with graceful degradation to title matching.
- `MruStore` / `EntryList` lifetimes are deliberately reasoned about; the daemon
  rebuilds the list per summon so the command target reflects the frontmost
  window at summon time.

---

## Suggested follow-ups (in priority order)

1. Resolve the matcher ordering divergence + reconcile the core docs/test
   comment (the only documented "bug"). *Being handled on Linux, where the
   GNOME `search` path is corrected to match the macOS two-tier ordering and
   the docs updated.*
2. ~~Fix the stale `app/macos/README.md:24` accessor claim.~~ — **done this
   session.** Generalized to "the `lofi_entries_get_*` accessors" + a pointer to
   the authoritative FFI list in `app/core/README.md`.
3. ~~Add a Swift formatter/linter~~ — **done this session.** Adopted
   `swift-format` (Apple's, bundled in Xcode) via `app/macos/check.sh` +
   `app/macos/.swift-format`; reformatted the 7 affected files; `./check.sh` is
   green and the lib still builds.
4. ~~Tidy the LOW Swift items~~ — **done this session.** `WindowDiscovery` cast
   fixed; `WindowControl` cast was a false positive (reverted + documented);
   `Hotkey` `OSStatus` now logged; dead `PanelController.isVisible` removed;
   `windowAux` asymmetry commented. The `NSApp.activate(ignoringOtherApps:)` call
   is left as-is per the analysis above (correct API, no warning). Build + swift-
   format gate green.
