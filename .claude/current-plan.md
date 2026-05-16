# Symlink regression test for the gatherer

## Summary

Add **one** new `#[test]` function to `/home/jplein/Git/jplein/lofi/app/gnome/tests/apps.rs`:

```rust
#[test]
fn gather_applications_follows_symlinks_to_desktop_files() { ... }
```

This pins the fix for the symlink-following bug. The existing integration test only writes regular files, so the symlink path is unexercised.

## What to change

### Modify `/home/jplein/Git/jplein/lofi/app/gnome/tests/apps.rs`

**Add one import** alongside the existing `std` imports:
```rust
use std::os::unix::fs::symlink;
```
(Linux-only; the `lofi-gnome` crate is already Linux-only — no `cfg(unix)` gate.)

**Append the new test** at the end of the file.

### Fixture layout (one `tempdir()`)

- `temp/targets/linked.desktop` — real file written via the existing `write_desktop(&targets, "linked.desktop", "Linked App", "true", "test-icon-linked")`.
- `temp/links/` — created with `fs::create_dir_all`. The dir that will be scanned.
  - `temp/links/linked.desktop` — symlink to **absolute** `temp/targets/linked.desktop`.
  - `temp/links/missing.desktop` — symlink to a path that does NOT exist (e.g. `temp/targets/does_not_exist.desktop`). The target need not exist for `symlink(2)` to succeed.

### Invocation

Pass only the `links_dir`:
```rust
let dirs: Vec<PathBuf> = vec![links_dir.clone()];
let apps = gather_applications(&dirs);
```

Excluding `targets_dir` from the input is the key design choice. If the gatherer regresses back to `DirEntry::file_type().is_file()`, the live symlink (file_type = "symlink", not "file") will be filtered out and the test fails with `apps.len() == 0`.

### Assertions

```rust
assert_eq!(apps.len(), 1, "expected 1 app (the live symlink); got {apps:?}");
assert_eq!(apps[0].name, "Linked App", "name should round-trip from symlinked target; got {:?}", apps[0].name);
```

Two assertions. We don't re-assert `desktop_id`/`icon` semantics — those are pinned by the existing test. This test is narrowly about: does the gatherer follow symlinks, and does it skip dangling ones safely?

### Why this also covers the dangling-symlink branch

`Path::is_file()` calls `metadata()` (not `symlink_metadata()`), which traverses the symlink. For a dangling symlink, the stat fails, `is_file()` returns `false`, and the `continue` is taken before reaching `DesktopAppInfo::from_filename`. The `apps.len() == 1` assertion proves the dangling one was skipped (had it been treated as a file, gio would still reject it as None, but we'd want to know if the metadata branch threw — and the assertion would catch any panic from a broken implementation).

## Constraints

- One file modified. No new files, no Cargo.toml changes, no production code changes, no README changes.
- Reuse the existing `write_desktop(...)` helper exactly.
- Parallel-safe: no env mutation.
- `expect(...)` with descriptive messages, matching the file's existing style.

## Verification

From `/home/jplein/Git/jplein/lofi/app/`:

1. `cargo test -p lofi-gnome` — new test passes, existing test continues to pass.
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo fmt --all -- --check`

## Out of scope

- Symlink to a directory, symlink chain, relative-target symlink, symlink to a non-`.desktop` file.
- Refactoring the existing test or extracting helpers.
- README updates.
- Production code changes.
