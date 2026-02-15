use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const SVG_DIR_ENV: &str = "FUSE_SVG_DIR";
const DEV_MODE_ENV: &str = "FUSE_DEV_MODE";

type SvgCache = HashMap<(String, String), String>;

static SVG_CACHE: OnceLock<Mutex<SvgCache>> = OnceLock::new();

pub(crate) fn load_svg_inline(name: &str) -> Result<String, String> {
    let (base_dir, relative) = resolve_svg_path(name)?;
    if is_dev_mode() {
        return read_svg(&base_dir.join(&relative));
    }
    let cache_key = (
        base_dir.to_string_lossy().to_string(),
        relative.to_string_lossy().to_string(),
    );
    let cache = SVG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(svg) = guard.get(&cache_key) {
            return Ok(svg.clone());
        }
    }
    let svg = read_svg(&base_dir.join(&relative))?;
    if let Ok(mut guard) = cache.lock() {
        guard.insert(cache_key, svg.clone());
    }
    Ok(svg)
}

fn is_dev_mode() -> bool {
    std::env::var(DEV_MODE_ENV)
        .ok()
        .as_deref()
        .map(|value| value == "1")
        .unwrap_or(false)
}

fn resolve_svg_path(name: &str) -> Result<(PathBuf, PathBuf), String> {
    let mut relative = sanitize_svg_name(name)?;
    if relative.extension().is_none() {
        relative.set_extension("svg");
    }
    if relative
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| !ext.eq_ignore_ascii_case("svg"))
        .unwrap_or(true)
    {
        return Err("svg.inline only supports .svg files".to_string());
    }
    let base_dir = std::env::var(SVG_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("assets/svg"));
    Ok((base_dir, relative))
}

fn sanitize_svg_name(name: &str) -> Result<PathBuf, String> {
    let raw = name.trim();
    if raw.is_empty() {
        return Err("svg.inline expects a non-empty path".to_string());
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err("svg.inline path must be relative to assets/svg".to_string());
    }
    let mut clean = PathBuf::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err("svg.inline path traversal is not allowed".to_string());
        }
        clean.push(part);
    }
    if clean.as_os_str().is_empty() {
        return Err("svg.inline expects a non-empty path".to_string());
    }
    Ok(clean)
}

fn read_svg(path: &PathBuf) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("svg.inline missing file: {}", path.display()));
    }
    fs::read_to_string(path)
        .map_err(|err| format!("svg.inline failed to read {}: {err}", path.display()))
}
