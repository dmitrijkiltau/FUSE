# Fuse VS Code tools

This folder provides syntax highlighting for `.fuse` files and a built-in Fuse LSP client.

## Syntax highlighting

Install the extension from this folder:

```
code <path-to-fuse-repo>
```

Then in the VS Code Command Palette:
- **Developer: Install Extension from Location...**
- Select the `tools/vscode` directory inside the repo
- Reload

## LSP (diagnostics + navigation + completion + refactor)

This extension now starts `fuse-lsp` directly.
Semantic highlighting is enabled by default for `[fuse]`, so LSP token colors override
TextMate scopes when the server is available.

Current LSP feature baseline:

- diagnostics on open/change/close
- formatting
- hover + go-to-definition + references
- rename refactor
- completion/autocomplete
- semantic tokens + inlay hints
- code actions (unresolved import quick fixes, config-field scaffold quick fixes, organize imports)

### Local dev

1) Build the dist binaries (includes `fuse-lsp`):

```
scripts/build_dist.sh
```

2) Install extension dependencies:

```
cd tools/vscode
npm install
```

3) Install the extension from this folder (as above).

By default the extension looks for:

1. `tools/vscode/bin/<platform>/fuse-lsp` (if you bundle it)
2. `dist/fuse-lsp` in the current workspace folder (and parent folders)
3. `fuse-lsp` on `PATH`

You can override with the setting:

```
fuse.lspPath
```

To verify which binary is used, open the **Fuse LSP** output channel; it logs
`Using fuse-lsp: <path>`.

Notes:
- Run the LSP inside WSL (so it can access the repo and binaries).
- If VS Code asks for a workspace/root, use the repo root.

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

1. builds `dist/fuse-lsp` (unless `--skip-build`),
2. copies it to `tools/vscode/bin/<platform>/fuse-lsp` (or `.exe` on Windows),
3. verifies path resolution priority (`bundled > workspace dist > PATH`),
4. emits `dist/fuse-vscode-<platform>.tgz`.

### Release checklist

1. Run `./scripts/build_dist.sh --release`.
2. Run `./scripts/package_vscode_extension.sh --platform <platform> --release`.
3. Verify `tools/vscode/bin/<platform>/fuse-lsp` exists and is executable.
4. Verify `dist/fuse-vscode-<platform>.tgz` exists.
5. Install/test the packaged extension in a clean VS Code profile.
