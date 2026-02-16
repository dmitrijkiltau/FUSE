const fs = require("fs");
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");
const { resolveLspCommandForHost } = require("./lsp-path");

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
  const override = config.get("lspPath", "");
  const workspaceFolders = (vscode.workspace.workspaceFolders || []).map(
    (folder) => folder.uri.fsPath
  );
  const result = resolveLspCommandForHost({
    override,
    extensionPath: context.extensionPath,
    workspaceFolders,
    pathExists: fs.existsSync,
    platform: process.platform,
    arch: process.arch,
  });

  if (result.warning) {
    vscode.window.showErrorMessage(result.warning);
  }

  if (result.source === "override") {
    output.appendLine(`Using configured fuse.lspPath: ${result.command}`);
  } else if (result.source === "path") {
    output.appendLine("No bundled fuse-lsp found, falling back to PATH.");
  } else {
    output.appendLine(`Using fuse-lsp: ${result.command}`);
  }
  return result.command;
}

module.exports = {
  activate,
  deactivate,
};
