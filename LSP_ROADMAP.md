# LSP / Editor Support Roadmap

This roadmap tracks editor-facing capabilities for FUSE and their validation gates.

## Current baseline (implemented)

Server: `crates/fusec/src/bin/fuse-lsp.rs`

- diagnostics (`textDocument/publishDiagnostics`)
- formatting (`textDocument/formatting`)
- definition/hover/references
- rename refactor (`textDocument/rename`)
- code actions (quick fixes + organize imports)
- semantic tokens (full + range)
- inlay hints
- call hierarchy
- workspace symbols
- completion/autocomplete (`textDocument/completion`)

Client: `tools/vscode/`

- VS Code extension starts `fuse-lsp` with workspace/dist/bundled path resolution
- TextMate grammar for `.fuse` syntax highlighting
- semantic highlighting enabled by default for `[fuse]`

Validation:

- `scripts/cargo_env.sh cargo test -p fusec --test lsp_ux`
- `scripts/cargo_env.sh cargo test -p fusec`

## Scope contract for editor support

For the current phase, editor support means:

1. Diagnostics update on open/change/close.
2. Symbol navigation and refactor are safe (definition/references/rename).
3. Completion is available for language symbols and core builtins.
4. Semantic tokens and inlay hints remain stable across parser/sema changes.
5. VS Code extension can launch the server without custom glue code.

## Next improvements (planned)

1. Signature help for function and route calls.
2. Completion ranking tuned by lexical scope and expected type.
3. Dedicated completion tests for member chains and import-aware suggestions.
4. Optional code actions for common runtime errors (e.g., missing config/import scaffolding).
5. Extension packaging workflow for platform-specific bundled `fuse-lsp` binaries.
