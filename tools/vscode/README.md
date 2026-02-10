# Fuse VS Code tools

This folder provides syntax highlighting for `.fuse` files and a built-in Fuse LSP client.

## Syntax highlighting

Install the extension from this folder (WSL):

```
code /home/dima/Projects/fuse
```

Then in the VS Code Command Palette:
- **Developer: Install Extension from Location...**
- Select `/home/dima/Projects/fuse/tools/vscode`
- Reload

## LSP (diagnostics + formatting + UX)

This extension now starts `fuse-lsp` directly.
Semantic highlighting is enabled by default for `[fuse]`, so LSP token colors override
TextMate scopes when the server is available.

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

### Shipping the LSP binary

For packaging, copy the platform binary into:

```
tools/vscode/bin/<platform>/fuse-lsp
```

Examples:

```
tools/vscode/bin/linux-x64/fuse-lsp
tools/vscode/bin/macos-arm64/fuse-lsp
tools/vscode/bin/windows-x64/fuse-lsp.exe
```
