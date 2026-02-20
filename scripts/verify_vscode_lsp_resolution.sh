#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ROOT="$ROOT" node <<'NODE'
const path = require("path");
const {
  resolveLspCommandForHost,
  platformDirName,
} = require(path.join(process.env.ROOT, "tools/vscode/lsp-path.js"));

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

const ext = "/ext";
const ws = ["/ws/project"];
const exe = "fuse-lsp";
const bundled = "/ext/bin/linux-x64/fuse-lsp";
const dist = "/ws/project/dist/fuse-lsp";

assert(
  platformDirName("linux", "x64") === "linux-x64",
  "expected linux-x64 platform dir"
);
assert(
  platformDirName("darwin", "arm64") === "macos-arm64",
  "expected macos-arm64 platform dir"
);
assert(
  platformDirName("win32", "x64") === "windows-x64",
  "expected windows-x64 platform dir"
);

{
  const exists = new Set([bundled, dist]);
  const result = resolveLspCommandForHost({
    override: "",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: (p) => exists.has(p),
    platform: "linux",
    arch: "x64",
  });
  assert(result.command === bundled, "bundled binary should win over workspace dist");
}

{
  const exists = new Set([dist]);
  const result = resolveLspCommandForHost({
    override: "",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: (p) => exists.has(p),
    platform: "linux",
    arch: "x64",
  });
  assert(result.command === dist, "workspace dist should win when bundled is missing");
}

{
  const result = resolveLspCommandForHost({
    override: "",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: () => false,
    platform: "linux",
    arch: "x64",
  });
  assert(result.command === exe, "should fall back to PATH fuse-lsp");
  assert(result.source === "path", "PATH fallback should mark source=path");
}

{
  const result = resolveLspCommandForHost({
    override: "",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: () => false,
    platform: "win32",
    arch: "x64",
  });
  assert(result.command === "fuse-lsp.exe", "win32 fallback should use fuse-lsp.exe");
}

{
  const result = resolveLspCommandForHost({
    override: "/custom/fuse-lsp",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: (p) => p === "/custom/fuse-lsp",
    platform: "linux",
    arch: "x64",
  });
  assert(result.command === "/custom/fuse-lsp", "valid override should win");
  assert(result.source === "override", "valid override should mark source=override");
}

console.log("vscode lsp resolution checks passed");
NODE
