use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::Manifest;

pub fn run_build(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    app: Option<&str>,
    clean: bool,
    aot: bool,
    release: bool,
    strict_architecture: bool,
) -> i32 {
    if clean {
        if let Err(err) = super::clean_build_dir(manifest_dir) {
            super::emit_cli_error(&err);
            return 1;
        }
        return 0;
    }
    if let Err(err) = super::assets::run_before_build_hook(manifest, manifest_dir) {
        super::emit_cli_error(&err);
        return 1;
    }
    if let Err(err) = super::assets::run_asset_pipeline(manifest, manifest_dir) {
        super::emit_cli_error(&err);
        return 1;
    }
    let configured_native_bin =
        manifest.and_then(|m| m.build.as_ref().and_then(|b| b.native_bin.clone()));
    let aot_enabled = aot || release;
    let aot_out = configured_native_bin.or_else(|| {
        if aot_enabled {
            Some(super::aot::default_aot_output_path())
        } else {
            None
        }
    });
    if aot_out.is_some() {
        super::emit_aot_build_progress(1, "compile program");
    }
    let artifacts =
        match super::aot::compile_artifacts(entry, manifest_dir, deps, strict_architecture) {
            Ok(artifacts) => artifacts,
            Err(err) => {
                super::emit_cli_error(&err);
                return 1;
            }
        };
    if aot_out.is_some() {
        super::emit_aot_build_progress(2, "write cache artifacts");
    }
    if let Err(err) = super::aot::write_compiled_artifacts(manifest_dir, &artifacts) {
        super::emit_cli_error(&err);
        return 1;
    }
    if let Some(native_bin) = aot_out {
        if let Err(err) = super::aot::write_native_binary(
            manifest_dir,
            &artifacts.native,
            app,
            &native_bin,
            release,
        ) {
            super::emit_cli_error(&err);
            return 1;
        }
    }
    let openapi_out = manifest.and_then(|m| m.build.as_ref().and_then(|b| b.openapi.clone()));
    let Some(out_path) = openapi_out else {
        return 0;
    };
    let out_path = {
        let path = PathBuf::from(&out_path);
        if path.is_absolute() {
            path
        } else if let Some(dir) = manifest_dir {
            dir.join(path)
        } else {
            match env::current_dir() {
                Ok(dir) => dir.join(path),
                Err(err) => {
                    super::emit_cli_error(&format!("cwd error: {err}"));
                    return 1;
                }
            }
        }
    };
    if let Err(err) = write_openapi(entry, &out_path, deps) {
        super::emit_cli_error(&err);
        return 1;
    }
    0
}

pub fn run_project_check(
    entry: &Path,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    strict_architecture: bool,
) -> i32 {
    let cache_meta = super::load_check_ir_meta(manifest_dir, strict_architecture);
    if let Some(meta) = cache_meta.as_ref() {
        if super::ir_meta_base_is_valid(meta, manifest_dir)
            && super::check_meta_files_unchanged(meta)
        {
            return 0;
        }
    }

    let src = match fs::read_to_string(entry) {
        Ok(src) => src,
        Err(err) => {
            super::emit_cli_error(&format!("failed to read {}: {err}", entry.display()));
            return 1;
        }
    };
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        super::emit_diags_with_fallback(&diags, Some((entry, &src)));
        return 1;
    }

    let current_meta = match super::build_ir_meta(&registry, manifest_dir) {
        Ok(meta) => meta,
        Err(err) => {
            super::emit_cli_error(&err);
            return 1;
        }
    };
    let changed = super::changed_modules_since_meta(
        &registry,
        &current_meta,
        cache_meta.as_ref(),
        manifest_dir,
    );
    let affected = super::affected_modules_for_incremental_check(&registry, &changed);

    let mut files = Vec::new();
    for id in affected {
        if let Some(unit) = registry.modules.get(&id) {
            files.push(unit.path.clone());
        }
    }
    files.sort();
    files.dedup();

    let mut had_errors = false;
    for file in files {
        if super::is_virtual_module_path(&file) {
            continue;
        }
        let src = match fs::read_to_string(&file) {
            Ok(src) => src,
            Err(err) => {
                super::emit_cli_error(&format!("failed to read {}: {err}", file.display()));
                return 1;
            }
        };
        let (registry, diags) = fusec::load_program_with_modules_and_deps(&file, &src, deps);
        if !diags.is_empty() {
            super::emit_diags_with_fallback(&diags, Some((&file, &src)));
            had_errors = true;
            continue;
        }
        let (_analysis, diags) = fusec::sema::analyze_registry_with_options(
            &registry,
            fusec::sema::AnalyzeOptions {
                strict_architecture,
            },
        );
        if !diags.is_empty() {
            super::emit_diags_with_fallback(&diags, Some((&file, &src)));
            had_errors = true;
        }
    }

    if !had_errors {
        if let Err(err) =
            super::write_check_ir_meta(manifest_dir, strict_architecture, &current_meta)
        {
            super::emit_cli_warning(&format!("failed to update check cache: {err}"));
        }
    }

    if had_errors { 1 } else { 0 }
}

pub fn run_project_fmt(entry: &Path, deps: &HashMap<String, PathBuf>) -> i32 {
    let files = match collect_project_files(entry, deps) {
        Ok(files) => files,
        Err(err) => {
            super::emit_cli_error(&err);
            return 1;
        }
    };
    for file in files {
        let src = match fs::read_to_string(&file) {
            Ok(src) => src,
            Err(err) => {
                super::emit_cli_error(&format!("failed to read {}: {err}", file.display()));
                return 1;
            }
        };
        let formatted = fusec::format::format_source(&src);
        if formatted != src {
            if let Err(err) = fs::write(&file, formatted) {
                super::emit_cli_error(&format!("failed to write {}: {err}", file.display()));
                return 1;
            }
        }
    }
    0
}

fn collect_project_files(
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let src = fs::read_to_string(entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        super::emit_diags(&diags);
        return Err("formatting aborted due to parse/sema errors".to_string());
    }
    let mut files = BTreeSet::new();
    for unit in registry.modules.values() {
        if unit.path.exists() {
            files.insert(unit.path.clone());
        }
    }
    if files.is_empty() {
        files.insert(entry.to_path_buf());
    }
    Ok(files.into_iter().collect())
}

fn write_openapi(
    entry: &Path,
    out_path: &Path,
    deps: &HashMap<String, PathBuf>,
) -> Result<(), String> {
    let src = fs::read_to_string(entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        super::emit_diags(&diags);
        return Err("build failed".to_string());
    }
    let json = fusec::openapi::generate_openapi(&registry)
        .map_err(|err| format!("openapi error: {err}"))?;
    if let Some(parent) = out_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
    }
    fs::write(out_path, json)
        .map_err(|err| format!("failed to write {}: {err}", out_path.display()))?;
    Ok(())
}
