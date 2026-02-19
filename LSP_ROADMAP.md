# LSP / Editor Support Roadmap

This roadmap tracks editor-facing capabilities for FUSE and their validation gates.

## Current baseline (implemented)

Server: `crates/fusec/src/bin/fuse-lsp.rs`

- diagnostics (`textDocument/publishDiagnostics`)
- formatting (`textDocument/formatting`)
- definition/hover/references
- rename refactor (`textDocument/rename`)
- rename refactor safety (`textDocument/prepareRename`)
- code actions:
  - unresolved symbol import quick fixes
  - missing config-field scaffold quick fixes (`unknown field <x> on <Config>`)
  - `source.organizeImports` with idempotent ordering behavior
- semantic tokens (full + range)
- inlay hints
- call hierarchy
- workspace symbols
- completion/autocomplete (`textDocument/completion`)
- signature help (`textDocument/signatureHelp`)
- workspace-index cache with invalidation on doc/root updates (reduces repeated workspace rebuilds)
- shared workspace snapshot cache for diagnostics + index requests (single workspace load per document revision)
- manifest-rooted entry resolution for non-entry files in workspace projects (improves cross-file cache reuse)
- fine-grained module-level cache patching for non-structural edits with structural-change fallback reloads
- dependency-graph-aware partial relinking for import/export shape changes when targets are already in the workspace graph
- incremental module loading for newly introduced local import paths during relink (avoids full-reload fallback for new in-workspace files)
- incremental relink support for newly introduced `dep:` import paths (avoids full-reload fallback for dependency modules present on disk)
- incremental materialization of `std.Error` during relink when newly introduced in edited modules (avoids pseudo-module fallback reload)
- cancellation handling validated for request bursts (`$/cancelRequest` contract)
- responsiveness budgets validated for large multi-file completion workloads

Client: `tools/vscode/`

- VS Code extension starts `fuse-lsp` with workspace/dist/bundled path resolution
- scriptable packaging workflow: `scripts/package_vscode_extension.sh`
- resolver verification gate: `scripts/verify_vscode_lsp_resolution.sh`
- TextMate grammar for `.fuse` syntax highlighting
- semantic highlighting enabled by default for `[fuse]`

Validation:

- `scripts/cargo_env.sh cargo test -p fusec --test lsp_ux`
- `scripts/cargo_env.sh cargo test -p fusec --test lsp_code_actions`
- `scripts/cargo_env.sh cargo test -p fusec --test lsp_workspace_incremental`
- `scripts/cargo_env.sh cargo test -p fusec --test lsp_perf_reliability`
- `scripts/verify_vscode_lsp_resolution.sh`
- `scripts/lsp_suite.sh`
- `scripts/cargo_env.sh cargo test -p fusec`
- `scripts/release_smoke.sh`

## Scope contract for editor support

For the current phase, editor support means:

1. Diagnostics update on open/change/close.
2. Symbol navigation and refactor are safe (definition/references/rename).
3. Completion is available for language symbols and core builtins.
4. Semantic tokens and inlay hints remain stable across parser/sema changes.
5. VS Code extension can launch the server without custom glue code.

## Next improvements (planned)

1. Extend dependency-root parsing coverage for additional manifest dependency syntaxes if/when they become part of the package spec surface.
