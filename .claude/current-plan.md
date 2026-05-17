# Dedupe `gather_applications` by `desktop_id` (first-wins, XDG shadowing)

## Context

`apps::gather_applications` walks every XDG directory returned by `application_directories()` and pushes every `.desktop` it finds. When the same `desktop_id` exists in multiple directories (e.g. Ghostty installed both via the Nix system profile and via `~/.local/share/applications`, or a Flatpak whose exports dir overlaps the user's local apps dir), the launcher list shows the app twice.

XDG convention says earlier entries on the search path **shadow** later ones by `desktop_id`. `application_directories()` already orders the path correctly: `$XDG_DATA_HOME` (or `$HOME/.local/share`) first, then each `$XDG_DATA_DIRS` entry. The gatherer just needs to honor that ordering.

## Change

### `app/gnome/src/apps.rs`

In `gather_applications`, track a `HashSet<String>` of seen `desktop_id`s. After resolving the canonical `desktop_id` for an entry, skip the push when the id is already in the set; otherwise insert and push.

The check must happen *after* the canonical `desktop_id` is computed (so e.g. a `foo.desktop` and a `foo.desktop` symlink in different dirs both collapse to the same canonical id), and *after* the `should_show()` / file-type / `DesktopAppInfo::from_filename` filters (so a hidden duplicate doesn't preempt a visible one — though in practice freedesktop hides per `desktop_id`, not per dir).

Because directories are walked in path order, the first directory listed in `dirs` wins. That preserves the XDG shadowing semantics and matches what the user expects.

### `app/gnome/tests/apps.rs`

Add one new integration test, `gather_applications_dedupes_by_desktop_id_first_wins`:

- Two temp dirs (`data_home_apps`, `usr_share_apps`), passed in that order to `gather_applications`.
- Write `ghostty.desktop` into both dirs with **different** `Name` and `Icon` values (e.g. `Name=Ghostty User`, `Icon=ghostty-user` in `data_home_apps`; `Name=Ghostty System`, `Icon=ghostty-system` in `usr_share_apps`).
- Also write a unique `.desktop` into each dir so the assertion is not trivially equivalent to "just look at dir 1".
- Assert: exactly one entry has `desktop_id == "ghostty.desktop"`, and its `name == "Ghostty User"` and `icon == Some("ghostty-user")` (i.e. the first dir's copy survived; the second was suppressed).
- Assert total count is 3 (one Ghostty + the two unique apps).

The existing two tests stay green because they use disjoint `desktop_id`s across dirs.

### `app/gnome/README.md`

Extend the `apps` bullet to note the dedup behavior — one short clause: `gather_applications` dedupes by `desktop_id`, first directory wins, following XDG shadowing. Mention the rationale briefly (same app installed in two prefixes shouldn't appear twice).

## Out of scope

- A configurable allow-duplicates mode.
- Surfacing which directory each `Application` came from.
- Merging field-by-field across duplicates (e.g. taking the icon from one and the name from the other) — first-wins is the XDG convention and matches every other launcher.
- Deduping `Window` entries against `Application` entries (intentional design from the prior task: an app and its windows are separate entries).

## Verification

- `cargo test -p lofi-gnome` — 3 integration tests pass (2 existing + 1 new).
- `cargo test --workspace` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `nix build` — clean.
- **Manual**: re-run the launcher, type "Ghostty", verify only one entry appears.
