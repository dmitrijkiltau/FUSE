# Project agent instructions

## Build and test

- Always run cargo commands through `scripts/cargo_env.sh` to avoid cross-device link errors.
- Default test command: `scripts/cargo_env.sh cargo test -p fusec`.
- Docs site lives in `docs/` (Astro + Starlight). Use `npm run build` from `docs/` for a successful build preview.

## Specs and fixtures

- Keep language specs in sync when semantics change:
  - `fuse.md`
  - `fls.md`
  - `scope.md`
  - `runtime.md`
- Parser fixtures live in `crates/fusec/tests/parser_fixtures.rs`.
- Semantic analysis golden tests live in `crates/fusec/tests/sema_golden.rs`.
