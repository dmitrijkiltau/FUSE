const path = require("path");

function platformDirName(platform = process.platform, arch = process.arch) {
  let os;
  if (platform === "win32") {
    os = "windows";
  } else if (platform === "darwin") {
    os = "macos";
  } else if (platform === "linux") {
    os = "linux";
  } else {
    return null;
  }
  const archName = arch === "x64" ? "x64" : arch === "arm64" ? "arm64" : arch;
  return `${os}-${archName}`;
}

function workspaceDistCandidates(workspaceFolders, exe, maxDepth = 8) {
  const seen = new Set();
  const out = [];
  for (const folder of workspaceFolders || []) {
    let current = folder;
    for (let i = 0; i < maxDepth; i++) {
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

function resolveLspCommandForHost(options) {
  const override = options.override || "";
  const extensionPath = options.extensionPath || "";
  const workspaceFolders = options.workspaceFolders || [];
  const pathExists = options.pathExists;
  const platform = options.platform || process.platform;
  const arch = options.arch || process.arch;

  if (typeof pathExists !== "function") {
    throw new Error("resolveLspCommandForHost requires pathExists");
  }

  const exe = platform === "win32" ? "fuse-lsp.exe" : "fuse-lsp";
  const candidates = [];
  const platformDir = platformDirName(platform, arch);
  if (platformDir) {
    candidates.push(path.join(extensionPath, "bin", platformDir, exe));
  }
  candidates.push(path.join(extensionPath, "bin", exe));
  candidates.push(...workspaceDistCandidates(workspaceFolders, exe));

  if (override) {
    if (pathExists(override)) {
      return {
        command: override,
        source: "override",
        warning: null,
        triedCandidates: candidates,
      };
    }
    // Keep compatibility with current behavior: invalid override reports an error
    // but still falls through to bundled/dist/PATH lookup.
    const result = pickFirstExisting(candidates, pathExists, exe);
    return {
      command: result.command,
      source: result.source,
      warning: `fuse.lspPath not found: ${override}`,
      triedCandidates: candidates,
    };
  }

  const result = pickFirstExisting(candidates, pathExists, exe);
  return {
    command: result.command,
    source: result.source,
    warning: null,
    triedCandidates: candidates,
  };
}

function pickFirstExisting(candidates, pathExists, fallbackExe) {
  for (const candidate of candidates) {
    if (pathExists(candidate)) {
      return {
        command: candidate,
        source: "candidate",
      };
    }
  }
  return {
    command: fallbackExe,
    source: "path",
  };
}

module.exports = {
  platformDirName,
  workspaceDistCandidates,
  resolveLspCommandForHost,
};
