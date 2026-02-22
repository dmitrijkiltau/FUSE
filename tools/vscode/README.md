# Fuse VS Code Extension

Syntax highlighting and LSP integration for `.fuse` files.

## Installation

Download the `.vsix` for your platform from the
[GitHub release](https://github.com/dmitrijkiltau/FUSE/releases) page, then:

```bash
# Install the extension package
code --install-extension fuse-vscode-linux-x64.vsix
```

Alternatively, install from source:

1. Open the VS Code Command Palette
2. Run **Developer: Install Extension from Location...**
3. Select the `tools/vscode` directory inside the repo
4. Reload

## LSP features

The extension starts the `fuse-lsp` binary automatically. Semantic highlighting is
enabled by default, so LSP token colors override TextMate scopes when the server is
available.

Supported capabilities:

- Diagnostics (on open, change, and close)
- Formatting
- Hover, go-to-definition, and references
- Rename
- Completion / autocomplete
- Semantic tokens and inlay hints
- Code actions (unresolved import fixes, config-field scaffolding, organize imports)

### Binary resolution

The extension searches for `fuse-lsp` in order:

1. `tools/vscode/bin/<platform>/fuse-lsp` (if you bundle it)
2. `dist/fuse-lsp` (or `dist/fuse-lsp.exe` on Windows) in the current workspace folder (and parent folders)
3. `fuse-lsp` on `PATH`

Override with the `fuse.lspPath` setting. The **Fuse LSP** output channel logs
the resolved binary path on startup.

### Local development

```bash
# Build dist binaries (includes fuse-lsp)
./scripts/build_dist.sh

# Install extension dependencies
cd tools/vscode && npm install
```

Then install the extension as described above.

> On WSL, run the LSP inside WSL so it can access the repo and binaries.

### Packaging workflow

Build and package the extension payload (including bundled `fuse-lsp`):

```
./scripts/package_vscode_extension.sh --platform linux-x64
```

Release-mode build + package:

```
./scripts/package_vscode_extension.sh --platform linux-x64 --release
```

This script:

1. builds `dist/fuse-lsp[.exe]` (unless `--skip-build`),
2. copies it to `tools/vscode/bin/<platform>/fuse-lsp` (or `.exe` on Windows),
3. verifies path resolution priority (`bundled > workspace dist > PATH`),
4. emits `dist/fuse-vscode-<platform>.vsix`,
5. validates VSIX package contents and bundled binary integrity.

### Release checklist

1. Run `./scripts/build_dist.sh --release`.
2. Run `./scripts/package_vscode_extension.sh --platform <platform> --release`.
3. Verify `tools/vscode/bin/<platform>/fuse-lsp` exists and is executable.
4. Verify `dist/fuse-vscode-<platform>.vsix` exists.
5. Run `./scripts/generate_release_checksums.sh` and publish `dist/SHA256SUMS`.
6. Install/test the packaged extension in a clean VS Code profile.
