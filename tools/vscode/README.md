# Fuse VS Code tools

This folder provides syntax highlighting for `.fuse` files and guidance for wiring the Fuse LSP.

## Syntax highlighting

Install the extension from this folder (WSL):

```
code /home/dima/Projects/fuse
```

Then in the VS Code Command Palette:
- **Developer: Install Extension from Location...**
- Select `/home/dima/Projects/fuse/tools/vscode`
- Reload

## LSP (diagnostics + formatting)

`fuse-lsp` runs as a stdio LSP server. Use any VS Code LSP client extension that supports
configuring a custom stdio server. Configure it to launch:

- Command: `/home/dima/Projects/fuse/scripts/fuse`
- Args: `lsp`
- Language ID: `fuse`

Notes:
- Run the LSP inside WSL (so it can access the repo and `scripts/fuse`).
- If the client asks for a workspace/root, use the repo root.

If you want, we can add a dedicated VS Code extension entry point that spawns `fuse-lsp`
without requiring a generic LSP client.
