## READMEs

The READMEs are the source of truth in this repo.

- If the READMEs disagree with the code, that's a bug, and either the README needs to be updated or the code does
- Maintain thorough READMEs about the repo
    - Each directory maintains READMEs about its contents
- It is more important to document why something was implemented in a certain way than to document how it works

## Rust checks

The Rust toolchain comes from a different place on each platform, so the
commands differ. **Bazel is macOS-only** — on Linux the toolchain (and the
reproducible build) come from Nix, and Bazel is not installed at all.

- **Linux** — toolchain via direnv + `flake.nix` (Crane). Run with Cargo from
  `app/`. These cover the whole workspace, including the Linux-only `gnome`
  crate:
    - `cargo test` (add `-p lofi-core --features ffi` to also run the FFI tests)
    - `cargo clippy --all-targets`
    - `cargo fmt --check`
- **macOS** — toolchain via Bazel; clippy/rustfmt are the rules_rust toolchain's
  own binaries, so there is no parallel cargo/rustup install to keep in sync
  (cargo is editor-tooling only here — see `app/README.md`). One command does
  everything:
    - `bazelisk test //app/...` — compiles every Bazel-built Rust target, runs
      `//app/core:clippy` (warnings promoted to errors) and `//app/core:rustfmt`,
      and runs the unit/integration tests. Only `core` builds under Bazel; the
      `gnome` crate is Linux-only.

- Before considering a task done:
    - The Rust code must compile
    - Linter checks with clippy must pass
    - Linter checks with rustfmt must pass
    - The Rust tests must pass

## TypeScript checks

The GNOME Shell extension (`extension/gnome`) is written in TypeScript. Run these from `extension/gnome` — `npm run check` runs all three in sequence.

- Before considering a task done:
    - Type checking with `tsc --noEmit` must pass (`npm run typecheck`)
    - Linter checks with eslint must pass (`npm run lint`)
    - Formatter checks with prettier must pass (`npm run format:check`)
