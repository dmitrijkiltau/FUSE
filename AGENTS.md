# Project agent instructions

## Build and test

- Always run cargo commands through `scripts/cargo_env.sh` to avoid cross-device link errors.
- Default test command: `scripts/cargo_env.sh cargo test -p fusec`.
- Use `scripts/fuse` for CLI commands; dist binaries are for release/distribution only.

## Specs and fixtures

- Keep language specs/docs in sync when semantics or tooling change:
  - `README.md`
  - `fuse.md`
  - `fls.md`
  - `scope.md`
  - `runtime.md`
- Parser fixtures live in `crates/fusec/tests/parser_fixtures.rs`.
- Semantic analysis golden tests live in `crates/fusec/tests/sema_golden.rs`.
