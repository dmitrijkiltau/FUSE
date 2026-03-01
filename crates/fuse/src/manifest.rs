use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::{CommonArgs, Manifest};

pub fn load_manifest(
    manifest_override: Option<&Path>,
) -> Result<(Option<Manifest>, Option<PathBuf>), String> {
    let (manifest_path, manifest_dir) = if let Some(path) = manifest_override {
        if path.is_dir() {
            let file = path.join("fuse.toml");
            (Some(file), Some(path.to_path_buf()))
        } else {
            (
                Some(path.to_path_buf()),
                path.parent().map(|p| p.to_path_buf()),
            )
        }
    } else {
        let cwd = env::current_dir().map_err(|err| format!("cwd error: {err}"))?;
        let path = find_manifest(&cwd);
        let dir = path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        (path, dir)
    };

    let Some(path) = manifest_path else {
        return Ok((None, None));
    };
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let manifest: Manifest =
        toml::from_str(&content).map_err(|err| format!("invalid manifest: {err}"))?;
    Ok((Some(manifest), manifest_dir))
}

pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join("fuse.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        let Some(parent) = dir.parent() else {
            return None;
        };
        dir = parent;
    }
}

pub fn resolve_entry(
    common: &CommonArgs,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let entry = common
        .entry
        .clone()
        .or_else(|| manifest.and_then(|m| m.package.entry.clone()));
    let Some(entry) = entry else {
        return Err(
            "missing entry: pass a file path or set package.entry in fuse.toml".to_string(),
        );
    };
    let path = PathBuf::from(&entry);
    if path.is_absolute() {
        return Ok(path);
    }
    if let Some(dir) = manifest_dir {
        return Ok(dir.join(path));
    }
    let cwd = env::current_dir().map_err(|err| format!("cwd error: {err}"))?;
    Ok(cwd.join(path))
}
