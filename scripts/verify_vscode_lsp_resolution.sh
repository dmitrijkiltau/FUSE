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
const bundled = path.join(ext, "bin", "linux-x64", "fuse-lsp");
const dist = path.join(ws[0], "dist", "fuse-lsp");
const bundledWin = path.join(ext, "bin", "windows-x64", "fuse-lsp.exe");
const distWin = path.join(ws[0], "dist", "fuse-lsp.exe");

function normalizePath(raw) {
  return raw.replace(/\\/g, "/");
}

function makePathExists(paths) {
  const normalized = new Set(paths.map((p) => normalizePath(p)));
  return (candidate) => normalized.has(normalizePath(candidate));
}

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
  const result = resolveLspCommandForHost({
    override: "",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: makePathExists([bundledWin, distWin]),
    platform: "win32",
    arch: "x64",
  });
  assert(result.command === bundledWin, "windows bundled binary should win over workspace dist");
}

{
  const result = resolveLspCommandForHost({
    override: "",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: makePathExists([bundled, dist]),
    platform: "linux",
    arch: "x64",
  });
  assert(result.command === bundled, "bundled binary should win over workspace dist");
}

{
  const result = resolveLspCommandForHost({
    override: "",
    extensionPath: ext,
    workspaceFolders: ws,
    pathExists: makePathExists([dist]),
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
