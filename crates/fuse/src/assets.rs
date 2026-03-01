use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use fuse_rt::json as rt_json;

use super::{FUSE_ASSET_MAP_ENV, Manifest};

pub(crate) fn collect_files_by_extension(root: &Path, exts: &[&str], out: &mut BTreeSet<PathBuf>) {
    let mut dirs = VecDeque::new();
    dirs.push_back(root.to_path_buf());
    while let Some(dir) = dirs.pop_front() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skip = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| matches!(name, ".git" | ".fuse" | "target"))
                    .unwrap_or(false);
                if !skip {
                    dirs.push_back(path);
                }
                continue;
            }
            let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            if exts
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
            {
                out.insert(path);
            }
        }
    }
}

pub(crate) fn resolve_manifest_relative_path(base: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn manifest_base_dir(manifest_dir: Option<&Path>) -> Result<PathBuf, String> {
    match manifest_dir {
        Some(dir) => Ok(dir.to_path_buf()),
        None => env::current_dir().map_err(|err| format!("cwd error: {err}")),
    }
}

pub(crate) fn run_before_build_hook(
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
) -> Result<(), String> {
    let Some(command) = manifest
        .and_then(|m| m.assets.as_ref())
        .and_then(|assets| assets.hooks.as_ref())
        .and_then(|hooks| hooks.before_build.as_deref())
    else {
        return Ok(());
    };
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }
    let base = manifest_base_dir(manifest_dir)?;
    let mut cmd = shell_command(command);
    cmd.current_dir(&base);
    let output = cmd
        .output()
        .map_err(|err| format!("asset hook error: failed to run before_build hook: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else if !stdout.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        format!("exit status {}", output.status)
    };
    Err(format!("asset hook error: before_build failed: {detail}"))
}

fn shell_command(command: &str) -> ProcessCommand {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = ProcessCommand::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = ProcessCommand::new("sh");
        cmd.arg("-lc").arg(command);
        cmd
    }
}

pub(crate) fn run_asset_pipeline(
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
) -> Result<(), String> {
    let base = manifest_base_dir(manifest_dir)?;
    let Some(assets) = manifest.and_then(|m| m.assets.as_ref()) else {
        clear_asset_manifest_for_base(&base)?;
        return Ok(());
    };
    let Some(css) = assets.css.as_ref() else {
        clear_asset_manifest_for_base(&base)?;
        return Ok(());
    };
    let source = resolve_manifest_relative_path(&base, css);
    if !source.exists() {
        return Err(format!(
            "assets.css path does not exist: {}",
            source.display()
        ));
    }
    let hash_requested = assets.hash.unwrap_or(false);
    if !hash_requested {
        clear_asset_manifest_for_base(&base)?;
        return Ok(());
    }

    let static_root = resolve_static_root(manifest, &base);
    let css_outputs = collect_css_outputs(&source);
    let mut hashed_map = BTreeMap::new();
    for output in css_outputs {
        let hashed = hash_css_file(&output)?;
        let key = asset_manifest_key(&base, &static_root, &output);
        let value = asset_manifest_value(&base, &static_root, &hashed);
        hashed_map.insert(key, value);
    }
    write_asset_manifest_for_base(&base, &hashed_map)
}

fn asset_manifest_path(base: &Path) -> PathBuf {
    base.join(".fuse").join("assets-manifest.json")
}

fn clear_asset_manifest_for_base(base: &Path) -> Result<(), String> {
    let path = asset_manifest_path(base);
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|err| format!("failed to remove {}: {err}", path.display()))?;
    }
    unsafe {
        env::remove_var(FUSE_ASSET_MAP_ENV);
    }
    Ok(())
}

fn write_asset_manifest_for_base(
    base: &Path,
    map: &BTreeMap<String, String>,
) -> Result<(), String> {
    if map.is_empty() {
        return clear_asset_manifest_for_base(base);
    }
    let json = rt_json::JsonValue::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), rt_json::JsonValue::String(value.clone())))
            .collect(),
    );
    let encoded = rt_json::encode(&json);
    let path = asset_manifest_path(base);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(&path, &encoded)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    unsafe {
        env::set_var(FUSE_ASSET_MAP_ENV, encoded);
    }
    Ok(())
}

fn resolve_static_root(manifest: Option<&Manifest>, base: &Path) -> PathBuf {
    let static_dir = manifest
        .and_then(|m| m.serve.as_ref())
        .and_then(|serve| serve.static_dir.as_deref())
        .unwrap_or("public");
    resolve_manifest_relative_path(base, static_dir)
}

fn collect_css_outputs(root: &Path) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    if root.is_file() {
        if !is_hashed_css_path(root) {
            files.insert(root.to_path_buf());
        }
        return files.into_iter().collect();
    }
    if root.is_dir() {
        collect_files_by_extension(root, &["css"], &mut files);
        return files
            .into_iter()
            .filter(|path| !is_hashed_css_path(path))
            .collect();
    }
    Vec::new()
}

fn hash_css_file(path: &Path) -> Result<PathBuf, String> {
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid CSS file name: {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("css");
    remove_stale_hashed_variants(path, stem, ext)?;
    let digest = super::sha1_digest(&bytes);
    let hash = hash_hex_prefix(&digest, 5);
    let hashed_name = format!("{stem}.{hash}.{ext}");
    let hashed_path = path.with_file_name(hashed_name);
    if hashed_path.exists() {
        fs::remove_file(&hashed_path)
            .map_err(|err| format!("failed to replace {}: {err}", hashed_path.display()))?;
    }
    fs::rename(path, &hashed_path).map_err(|err| {
        format!(
            "failed to rename {} -> {}: {err}",
            path.display(),
            hashed_path.display()
        )
    })?;
    Ok(hashed_path)
}

fn remove_stale_hashed_variants(path: &Path, stem: &str, ext: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let prefix = format!("{stem}.");
    let entries = fs::read_dir(parent)
        .map_err(|err| format!("failed to read {}: {err}", parent.display()))?;
    for entry in entries.flatten() {
        let candidate = entry.path();
        if candidate == path || !candidate.is_file() {
            continue;
        }
        if candidate.extension().and_then(|value| value.to_str()) != Some(ext) {
            continue;
        }
        let Some(candidate_stem) = candidate.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(suffix) = candidate_stem.strip_prefix(&prefix) else {
            continue;
        };
        if !is_hex_suffix(suffix) {
            continue;
        }
        fs::remove_file(&candidate)
            .map_err(|err| format!("failed to remove {}: {err}", candidate.display()))?;
    }
    Ok(())
}

fn asset_manifest_key(base: &Path, static_root: &Path, path: &Path) -> String {
    let relative = path
        .strip_prefix(static_root)
        .or_else(|_| path.strip_prefix(base))
        .unwrap_or(path);
    normalize_asset_key(&path_to_forward_slashes(relative))
}

fn asset_manifest_value(base: &Path, static_root: &Path, path: &Path) -> String {
    let relative = path
        .strip_prefix(static_root)
        .or_else(|_| path.strip_prefix(base))
        .unwrap_or(path);
    let key = normalize_asset_key(&path_to_forward_slashes(relative));
    if key.is_empty() {
        "/".to_string()
    } else {
        format!("/{key}")
    }
}

fn path_to_forward_slashes(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_asset_key(raw: &str) -> String {
    raw.trim()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string()
}

fn is_hashed_css_path(path: &Path) -> bool {
    if !matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("css")
    ) {
        return false;
    }
    let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((_, suffix)) = stem.rsplit_once('.') else {
        return false;
    };
    is_hex_suffix(suffix)
}

fn is_hex_suffix(raw: &str) -> bool {
    raw.len() >= 6 && raw.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn hash_hex_prefix(bytes: &[u8], take: usize) -> String {
    let mut out = String::new();
    for byte in bytes.iter().take(take) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn apply_asset_manifest_env(manifest_dir: Option<&Path>) {
    let Ok(base) = manifest_base_dir(manifest_dir) else {
        unsafe {
            env::remove_var(FUSE_ASSET_MAP_ENV);
        }
        return;
    };
    let path = asset_manifest_path(&base);
    let map = fs::read_to_string(&path).ok();
    match map {
        Some(map) if !map.trim().is_empty() => unsafe {
            env::set_var(FUSE_ASSET_MAP_ENV, map);
        },
        _ => unsafe {
            env::remove_var(FUSE_ASSET_MAP_ENV);
        },
    }
}
