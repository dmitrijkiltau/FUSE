# Changelog

## 0.3.2 — 2026-02-22

### Changed

- Release metadata updated to `0.3.2`.
- Release packaging/docs now align with platform artifact matrix and checksum publication flow.
- Packaging guidance now explicitly covers Windows bundled-binary (`.exe`) handling.

## 0.2.0 — 2026-02-22

### Changed

- Packaging artifact switched to installable `.vsix` (`dist/fuse-vscode-<platform>.vsix`).
- Packaging workflow now validates VSIX contents and bundled platform binary integrity.

## 0.1.0 — 2026-02-21

Initial release, matching FUSE v0.1.0.

### Features

- TextMate grammar for `.fuse` syntax highlighting
- Built-in LSP client (starts `fuse-lsp` automatically via stdio)
- Semantic highlighting (overrides TextMate scopes when LSP is available)
- Diagnostics on open, change, and close
- Formatting
- Hover, go-to-definition, and references
- Rename
- Completion / autocomplete
- Semantic tokens and inlay hints
- Code actions (unresolved import fixes, config-field scaffolding, organize imports)

### Configuration

- `fuse.lspPath` — override the `fuse-lsp` binary location
- Auto-detection: bundled binary → `dist/fuse-lsp` in workspace → `PATH`
