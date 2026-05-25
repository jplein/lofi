import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import prettier from 'eslint-config-prettier';
import { defineConfig, globalIgnores } from 'eslint/config';

// Flat config (ESLint 9+). Kept intentionally close to the recommended
// presets: this is a small extension and the value is catching real mistakes
// (unused vars, accidental `any`, shadowing), not enforcing a bespoke style —
// formatting is Prettier's job (see `.prettierrc.json`).
export default defineConfig([
    // Never lint build output or dependencies.
    globalIgnores(['dist/', 'node_modules/']),

    // Baseline JS recommendations, then the TypeScript-aware ones.
    js.configs.recommended,
    tseslint.configs.recommended,

    {
        // GJS / GNOME Shell code reaches for ambient globals (`global`,
        // `globalThis`, ...) that ESLint's `no-undef` doesn't know about.
        // TypeScript already proves every identifier is defined using the
        // @girs ambient declarations (see `ambient.d.ts`), so we defer to it
        // and switch the rule off for `.ts` files — this is exactly what
        // typescript-eslint recommends, since a duplicate (and weaker) check
        // would only produce false positives here.
        files: ['**/*.ts'],
        rules: { 'no-undef': 'off' },
    },

    // Must stay last: turns off the ESLint rules that would otherwise fight
    // Prettier over formatting.
    prettier,
]);
