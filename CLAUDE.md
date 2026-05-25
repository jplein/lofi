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

## TypeScript checks

The GNOME Shell extension (`extension/gnome`) is written in TypeScript. Run these from `extension/gnome` — `npm run check` runs all three in sequence.

- Before considering a task done:
    - Type checking with `tsc --noEmit` must pass (`npm run typecheck`)
    - Linter checks with eslint must pass (`npm run lint`)
    - Formatter checks with prettier must pass (`npm run format:check`)
