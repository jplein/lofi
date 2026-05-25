## READMEs

The READMEs are the source of truth in this repo.

- If the READMEs disagree with the code, that's a bug, and either the README needs to be updated or the code does
- Maintain thorough READMEs about the repo
    - Each directory maintains READMEs about its contents
- It is more important to document why something was implemented in a certain way than to document how it works

## Rust checks

- Before considering a task done:
    - The Rust code must compile
    - Linter checks with clippy must pass
    - Linter checks with rustfmt must pass
    - The Rust tests must pass

See [app/README.md](app/README.md#checks) for how to reach the toolchain and run
these checks — the commands differ by platform (Linux uses Cargo, macOS uses
Bazel).

## Swift checks

- Before considering a task done:
    - Swift formatting and lint checks must pass

See [app/macos/README.md](app/macos/README.md#formatting--linting) for how to run
the Swift checks.

## TypeScript checks

- Before considering a task done:
    - Type checking with tsc must pass
    - Linter checks with eslint must pass
    - Formatter checks with prettier must pass

See [extension/gnome/README.md](extension/gnome/README.md#linting-and-formatting)
for how to run the TypeScript checks.
