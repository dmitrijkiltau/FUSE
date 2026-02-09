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

  const repoRoot = path.resolve(context.extensionPath, "..", "..");
  candidates.push(path.join(repoRoot, "dist", exe));

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

module.exports = {
  activate,
  deactivate,
};
