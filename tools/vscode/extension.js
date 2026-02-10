const path = require("path");
const fs = require("fs");
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function activate(context) {
  const output = vscode.window.createOutputChannel("Fuse LSP");
  const command = resolveServerCommand(context, output);
  if (!command) {
    output.show(true);
    return;
  }
  const serverOptions = {
    command,
    args: [],
    transport: TransportKind.stdio,
  };
  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "fuse" }],
    outputChannel: output,
  };
  client = new LanguageClient("fuse-lsp", "Fuse LSP", serverOptions, clientOptions);
  context.subscriptions.push(client.start());
}

function deactivate() {
  if (!client) return undefined;
  return client.stop();
}

function resolveServerCommand(context, output) {
  const config = vscode.workspace.getConfiguration("fuse");
  const override = config.get("lspPath");
  if (override) {
    if (fs.existsSync(override)) {
      output.appendLine(`Using configured fuse.lspPath: ${override}`);
      return override;
    }
    vscode.window.showErrorMessage(`fuse.lspPath not found: ${override}`);
  }

  const exe = process.platform === "win32" ? "fuse-lsp.exe" : "fuse-lsp";
  const candidates = [];

  const platformDir = platformDirName();
  if (platformDir) {
    candidates.push(context.asAbsolutePath(path.join("bin", platformDir, exe)));
  }
  candidates.push(context.asAbsolutePath(path.join("bin", exe)));

  // When installed, extensionPath points to VS Code's extension cache, not repo root.
  // Prefer dist binaries discovered from workspace folders (and their parent dirs).
  candidates.push(...workspaceDistCandidates(exe));

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      output.appendLine(`Using fuse-lsp: ${candidate}`);
      return candidate;
    }
  }

  output.appendLine("No bundled fuse-lsp found, falling back to PATH.");
  return exe;
}

function platformDirName() {
  let os;
  if (process.platform === "win32") {
    os = "windows";
  } else if (process.platform === "darwin") {
    os = "macos";
  } else if (process.platform === "linux") {
    os = "linux";
  } else {
    return null;
  }
  const arch = process.arch === "x64" ? "x64" : process.arch === "arm64" ? "arm64" : process.arch;
  return `${os}-${arch}`;
}

function workspaceDistCandidates(exe) {
  const folders = vscode.workspace.workspaceFolders || [];
  const seen = new Set();
  const out = [];
  for (const folder of folders) {
    let current = folder.uri.fsPath;
    for (let i = 0; i < 8; i++) {
      const candidate = path.join(current, "dist", exe);
      if (!seen.has(candidate)) {
        seen.add(candidate);
        out.push(candidate);
      }
      const parent = path.dirname(current);
      if (parent === current) {
        break;
      }
      current = parent;
    }
  }
  return out;
}

module.exports = {
  activate,
  deactivate,
};
