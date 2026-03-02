use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Manifest, ServeConfig};

pub(crate) fn configure_openapi_ui_env(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    dev_mode: bool,
) -> Result<(), String> {
    let serve = manifest.and_then(|m| m.serve.as_ref());
    let enabled = if dev_mode {
        serve.and_then(|cfg| cfg.openapi_ui).unwrap_or(true)
    } else {
        serve.and_then(|cfg| cfg.openapi_ui).unwrap_or(false)
    };
    if !enabled {
        unsafe {
            env::remove_var("FUSE_OPENAPI_JSON_PATH");
            env::remove_var("FUSE_OPENAPI_UI_PATH");
        }
        return Ok(());
    }
    let openapi_json = generate_openapi_json(entry, deps)?;
    let out_path = openapi_ui_spec_path(manifest_dir)?;
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(&out_path, openapi_json)
        .map_err(|err| format!("failed to write {}: {err}", out_path.display()))?;
    let ui_path = serve
        .and_then(|cfg| cfg.openapi_path.as_deref())
        .map(normalize_openapi_ui_route)
        .unwrap_or_else(|| "/docs".to_string());
    unsafe {
        env::set_var(
            "FUSE_OPENAPI_JSON_PATH",
            out_path.to_string_lossy().to_string(),
        );
        env::set_var("FUSE_OPENAPI_UI_PATH", ui_path);
    }
    Ok(())
}

fn generate_openapi_json(entry: &Path, deps: &HashMap<String, PathBuf>) -> Result<String, String> {
    let src = fs::read_to_string(entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        super::emit_diags(&diags);
        return Err("openapi ui setup failed".to_string());
    }
    fusec::openapi::generate_openapi(&registry).map_err(|err| format!("openapi error: {err}"))
}

fn openapi_ui_spec_path(manifest_dir: Option<&Path>) -> Result<PathBuf, String> {
    let base = match manifest_dir {
        Some(dir) => dir.to_path_buf(),
        None => env::current_dir().map_err(|err| format!("cwd error: {err}"))?,
    };
    Ok(base.join(".fuse").join("dev").join("openapi.json"))
}

fn normalize_openapi_ui_route(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "/docs".to_string();
    }
    let mut path = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    path
}

pub(crate) fn apply_serve_env(manifest: Option<&Manifest>, manifest_dir: Option<&Path>) {
    super::assets::apply_asset_manifest_env(manifest_dir);
    apply_svg_env(manifest_dir);
    let dev_mode = env::var("FUSE_DEV_MODE")
        .ok()
        .as_deref()
        .map(|value| value == "1")
        .unwrap_or(false);
    apply_vite_proxy_env(manifest, dev_mode);
    let serve = manifest.and_then(|m| m.serve.as_ref());
    let static_dir = resolve_static_dir_setting(manifest, serve, dev_mode);
    let static_index = serve.and_then(|cfg| cfg.static_index.as_ref());
    match static_dir {
        Some(static_dir) => {
            let mut resolved = PathBuf::from(static_dir);
            if resolved.is_relative() {
                if let Some(base) = manifest_dir {
                    resolved = base.join(resolved);
                }
            }
            unsafe {
                env::set_var("FUSE_STATIC_DIR", resolved.to_string_lossy().to_string());
            }
        }
        None => unsafe {
            env::remove_var("FUSE_STATIC_DIR");
        },
    }
    match static_index {
        Some(index) => unsafe {
            env::set_var("FUSE_STATIC_INDEX", index);
        },
        None => unsafe {
            env::remove_var("FUSE_STATIC_INDEX");
        },
    }
}

fn apply_svg_env(manifest_dir: Option<&Path>) {
    let base = manifest_dir
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let svg_dir = base.join("assets").join("svg");
    unsafe {
        env::set_var("FUSE_SVG_DIR", svg_dir.to_string_lossy().to_string());
    }
}

fn resolve_static_dir_setting<'a>(
    manifest: Option<&'a Manifest>,
    serve: Option<&'a ServeConfig>,
    dev_mode: bool,
) -> Option<&'a str> {
    if let Some(static_dir) = serve.and_then(|cfg| cfg.static_dir.as_deref()) {
        return Some(static_dir);
    }
    if dev_mode {
        return None;
    }
    manifest
        .and_then(|m| m.vite.as_ref())
        .and_then(|vite| vite.dist_dir.as_deref())
        .or_else(|| manifest.and_then(|m| m.vite.as_ref()).map(|_| "dist"))
}

fn apply_vite_proxy_env(manifest: Option<&Manifest>, dev_mode: bool) {
    if !dev_mode {
        unsafe {
            env::remove_var("FUSE_VITE_PROXY_URL");
        }
        return;
    }
    let Some(vite) = manifest.and_then(|m| m.vite.as_ref()) else {
        unsafe {
            env::remove_var("FUSE_VITE_PROXY_URL");
        }
        return;
    };
    let url = vite
        .dev_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or("http://127.0.0.1:5173");
    unsafe {
        env::set_var("FUSE_VITE_PROXY_URL", url);
    }
}

pub(crate) fn apply_dotenv(manifest_dir: Option<&Path>) {
    let mut path = PathBuf::from(".env");
    if let Some(dir) = manifest_dir {
        path = dir.join(".env");
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            super::emit_cli_warning(&format!("failed to read {}: {err}", path.display()));
            return;
        }
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let mut parts = line.splitn(2, '=');
        let key = match parts.next() {
            Some(key) => key.trim(),
            None => continue,
        };
        if key.is_empty() {
            continue;
        }
        let value = parts.next().unwrap_or("").trim();
        let value = if value.len() >= 2 {
            let bytes = value.as_bytes();
            if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
                || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            {
                &value[1..value.len() - 1]
            } else {
                value
            }
        } else {
            value
        };
        if env::var_os(key).is_some() {
            continue;
        }
        unsafe {
            env::set_var(key, value);
        }
    }
}

pub(crate) fn apply_default_config_path(manifest_dir: Option<&Path>) {
    if env::var_os("FUSE_CONFIG").is_some() {
        return;
    }
    let Some(dir) = manifest_dir else {
        return;
    };
    let path = dir.join("config.toml");
    if !path.exists() {
        return;
    }
    unsafe {
        env::set_var("FUSE_CONFIG", path.to_string_lossy().to_string());
    }
}
