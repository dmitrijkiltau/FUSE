use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::SystemTime;

use fuse_rt::error::{ValidationError, ValidationField};
use fuse_rt::json as rt_json;

use crate::cli_output::diagnostics_json_enabled;

pub(crate) struct BuildArtifacts {
    pub(crate) native: fusec::native::NativeProgram,
    pub(crate) meta: super::IrMeta,
}

#[derive(Default)]
struct RawProgramArgs {
    values: HashMap<String, Vec<String>>,
    bools: HashMap<String, bool>,
}

pub(crate) fn default_aot_output_path() -> String {
    if cfg!(windows) {
        ".fuse/build/program.aot.exe".to_string()
    } else {
        ".fuse/build/program.aot".to_string()
    }
}

pub(crate) fn write_native_binary(
    manifest_dir: Option<&Path>,
    program: &fusec::native::NativeProgram,
    app: Option<&str>,
    out_path: &str,
    release: bool,
) -> Result<(), String> {
    let build_dir = super::build_dir(manifest_dir)?;
    if !build_dir.exists() {
        fs::create_dir_all(&build_dir)
            .map_err(|err| format!("failed to create {}: {err}", build_dir.display()))?;
    }
    super::emit_aot_build_progress(3, "emit native object");
    let artifact = fusec::native::emit_object_for_app(program, app)?;
    let object_path = build_dir.join("program.o");
    fs::write(&object_path, &artifact.object)
        .map_err(|err| format!("failed to write {}: {err}", object_path.display()))?;
    let mut configs: Vec<fusec::ir::Config> = program.ir.configs.values().cloned().collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    let config_bytes =
        bincode::serialize(&configs).map_err(|err| format!("config encode failed: {err}"))?;
    let program_bytes =
        bincode::serialize(program).map_err(|err| format!("program encode failed: {err}"))?;
    let mut types: Vec<fusec::ir::TypeInfo> = program.ir.types.values().cloned().collect();
    types.sort_by(|a, b| a.name.cmp(&b.name));
    let type_bytes =
        bincode::serialize(&types).map_err(|err| format!("type encode failed: {err}"))?;
    let runner_path = build_dir.join("native_main.rs");
    super::emit_aot_build_progress(4, "write runner source");
    write_native_runner(
        &runner_path,
        &artifact.entry_symbol,
        &artifact.interned_strings,
        &program_bytes,
        &config_bytes,
        &type_bytes,
        &artifact.config_defaults,
        release,
    )?;
    let out_path = resolve_output_path(manifest_dir, out_path)?;
    if let Some(parent) = out_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
    }
    link_native_binary(&runner_path, &object_path, &out_path, release)?;
    Ok(())
}

fn resolve_output_path(manifest_dir: Option<&Path>, path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return Ok(path);
    }
    if let Some(dir) = manifest_dir {
        return Ok(dir.join(path));
    }
    let cwd = env::current_dir().map_err(|err| format!("cwd error: {err}"))?;
    Ok(cwd.join(path))
}

fn write_native_runner(
    path: &Path,
    entry_symbol: &str,
    interned_strings: &[String],
    program_bytes: &[u8],
    config_bytes: &[u8],
    type_bytes: &[u8],
    config_defaults: &[fusec::native::ConfigDefaultSymbol],
    release: bool,
) -> Result<(), String> {
    let interned = if interned_strings.is_empty() {
        "&[]".to_string()
    } else {
        let items: Vec<String> = interned_strings
            .iter()
            .map(|value| format!("{value:?}"))
            .collect();
        format!("&[{}]", items.join(", "))
    };
    let config_blob = if config_bytes.is_empty() {
        "&[]".to_string()
    } else {
        let bytes: Vec<String> = config_bytes.iter().map(|b| b.to_string()).collect();
        format!("&[{}]", bytes.join(", "))
    };
    let program_blob = if program_bytes.is_empty() {
        "&[]".to_string()
    } else {
        let bytes: Vec<String> = program_bytes.iter().map(|b| b.to_string()).collect();
        format!("&[{}]", bytes.join(", "))
    };
    let type_blob = if type_bytes.is_empty() {
        "&[]".to_string()
    } else {
        let bytes: Vec<String> = type_bytes.iter().map(|b| b.to_string()).collect();
        format!("&[{}]", bytes.join(", "))
    };
    let mut default_decls = String::new();
    let mut default_matches = String::new();
    for (idx, def) in config_defaults.iter().enumerate() {
        let fn_name = format!("fuse_default_{idx}");
        default_decls.push_str(&format!(
            "unsafe extern \"C\" {{\n    #[link_name = \"{}\"]\n    fn {fn_name}(args: *const NativeValue, out: *mut NativeValue, heap: *mut std::ffi::c_void) -> u8;\n}}\n\n",
            def.symbol
        ));
        default_matches.push_str(&format!(
            "        \"{}\" => call_native({fn_name}, heap),\n",
            def.name
        ));
    }
    if default_matches.is_empty() {
        default_matches.push_str("        _ => Err(format!(\"unknown config default {name}\")),\n");
    } else {
        default_matches.push_str("        _ => Err(format!(\"unknown config default {name}\")),\n");
    }
    let target_literal = format!("{:?}", super::BUILD_TARGET_FINGERPRINT);
    let rustc_literal = format!("{:?}", super::BUILD_RUSTC_FINGERPRINT);
    let cli_literal = format!("{:?}", super::BUILD_CLI_VERSION_FINGERPRINT);
    let contract_literal = format!("{:?}", super::AOT_SEMANTIC_CONTRACT_VERSION);
    let mode_literal = "\"aot\"";
    let profile_literal = if release { "\"release\"" } else { "\"debug\"" };
    let runtime_cache_version = fusec::native::CACHE_VERSION;
    let source = format!(
        r#"use fusec::interp::format_error_value;
use fusec::native::value::{{NativeHeap, NativeValue}};
use fusec::native::{{load_configs_for_binary, load_types_for_binary}};
use fusec::observability::{{classify_panic_payload, format_panic_message}};

type EntryFn = unsafe extern "C" fn(*const NativeValue, *mut NativeValue, *mut std::ffi::c_void) -> u8;

unsafe extern "C" {{
    #[link_name = "{entry_symbol}"]
    fn fuse_entry(
        args: *const NativeValue,
        out: *mut NativeValue,
        heap: *mut std::ffi::c_void,
    ) -> u8;
}}

{default_decls}

const INTERNED_STRINGS: &[&str] = {interned};
const PROGRAM_BYTES: &[u8] = {program_blob};
const CONFIG_BYTES: &[u8] = {config_blob};
const TYPE_BYTES: &[u8] = {type_blob};
const AOT_STARTUP_MODE: &str = {mode_literal};
const AOT_BUILD_PROFILE: &str = {profile_literal};
const AOT_BUILD_TARGET: &str = {target_literal};
const AOT_BUILD_RUSTC: &str = {rustc_literal};
const AOT_BUILD_CLI: &str = {cli_literal};
const AOT_RUNTIME_CACHE_VERSION: u32 = {runtime_cache_version};
const AOT_SEMANTIC_CONTRACT: &str = {contract_literal};

fn build_info_line() -> String {{
    format!(
        "mode={{}} profile={{}} target={{}} rustc={{}} cli={{}} runtime_cache={{}} contract={{}}",
        AOT_STARTUP_MODE,
        AOT_BUILD_PROFILE,
        AOT_BUILD_TARGET,
        AOT_BUILD_RUSTC,
        AOT_BUILD_CLI,
        AOT_RUNTIME_CACHE_VERSION,
        AOT_SEMANTIC_CONTRACT
    )
}}

fn sanitize_message(raw: &str) -> String {{
    raw.replace('\n', "\\n").replace('\r', "\\r")
}}

fn emit_fatal(class: &str, message: &str) {{
    eprintln!(
        "fatal: class={{}} pid={{}} message={{}} {{}}",
        class,
        std::process::id(),
        sanitize_message(message),
        build_info_line()
    );
}}

fn emit_startup_trace() {{
    if std::env::var("FUSE_AOT_STARTUP_TRACE").ok().as_deref() == Some("1") {{
        eprintln!("startup: pid={{}} {{}}", std::process::id(), build_info_line());
    }}
}}

fn env_truthy(key: &str) -> bool {{
    let Ok(raw) = std::env::var(key) else {{
        return false;
    }};
    let value = raw.trim().to_ascii_lowercase();
    value == "1" || value == "true" || value == "structured" || value == "json"
}}

fn apply_release_structured_log_default() {{
    if AOT_BUILD_PROFILE != "release" {{
        return;
    }}
    if std::env::var_os("FUSE_REQUEST_LOG").is_some() {{
        return;
    }}
    if !env_truthy("FUSE_AOT_REQUEST_LOG_DEFAULT") {{
        return;
    }}
    unsafe {{
        std::env::set_var("FUSE_REQUEST_LOG", "structured");
    }}
}}

fn call_native(entry: EntryFn, heap: &mut NativeHeap) -> Result<fusec::interp::Value, String> {{
    let mut out = NativeValue::null();
    let status = unsafe {{ entry(std::ptr::null(), &mut out, heap as *mut _ as *mut std::ffi::c_void) }};
    match status {{
        0 => out
            .to_value(heap)
            .ok_or_else(|| "native error".to_string()),
        1 => {{
            if let Some(value) = out.to_value(heap) {{
                Err(format_error_value(&value))
            }} else {{
                Err("native error".to_string())
            }}
        }}
        2 => {{
            if let Some(value) = out.to_value(heap) {{
                Err(value.to_string_value())
            }} else {{
                Err("native runtime error".to_string())
            }}
        }}
        _ => Err(format!("native runtime error (status {{status}})")),
    }}
}}

fn call_default(name: &str, heap: &mut NativeHeap) -> Result<fusec::interp::Value, String> {{
    match name {{
{default_matches}    }}
}}

fn load_configs(program: &fusec::native::NativeProgram, heap: &mut NativeHeap) -> Result<(), String> {{
    if CONFIG_BYTES.is_empty() {{
        return Ok(());
    }}
    let configs: Vec<fusec::ir::Config> =
        bincode::deserialize(CONFIG_BYTES).map_err(|err| format!("config decode failed: {{err}}"))?;
    load_configs_for_binary(
        configs.iter(),
        &program.ir.types,
        &program.ir.enums,
        heap,
        |name, heap| call_default(name, heap),
    )
}}

fn load_types(heap: &mut NativeHeap) -> Result<(), String> {{
    if TYPE_BYTES.is_empty() {{
        return Ok(());
    }}
    let types: Vec<fusec::ir::TypeInfo> =
        bincode::deserialize(TYPE_BYTES).map_err(|err| format!("type decode failed: {{err}}"))?;
    load_types_for_binary(types.iter(), heap)
}}

fn run_program() -> Result<(), String> {{
    if PROGRAM_BYTES.is_empty() {{
        return Err("missing native program metadata".to_string());
    }}
    let program: fusec::native::NativeProgram =
        bincode::deserialize(PROGRAM_BYTES).map_err(|err| format!("program decode failed: {{err}}"))?;
    let mut runtime_vm = fusec::native::NativeVm::new(&program);
    let _runtime_context = runtime_vm.enter_runtime_context();
    let mut heap = NativeHeap::new();
    for value in INTERNED_STRINGS {{
        heap.intern_string((*value).to_string());
    }}
    load_types(&mut heap)?;
    load_configs(&program, &mut heap)?;
    call_native(fuse_entry, &mut heap)?;
    Ok(())
}}

fn main() {{
    if std::env::var("FUSE_AOT_BUILD_INFO").ok().as_deref() == Some("1") {{
        println!("{{}}", build_info_line());
        return;
    }}
    apply_release_structured_log_default();
    emit_startup_trace();
    let status = match std::panic::catch_unwind(run_program) {{
        Ok(Ok(())) => 0,
        Ok(Err(err)) => {{
            emit_fatal("runtime_fatal", &err);
            1
        }}
        Err(payload) => {{
            let details = classify_panic_payload(payload.as_ref());
            let msg = format_panic_message(&details);
            emit_fatal("panic", &msg);
            101
        }}
    }};
    std::process::exit(status);
}}
"#
    );
    fs::write(path, source).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(())
}

fn link_native_binary(
    runner: &Path,
    object: &Path,
    out_path: &Path,
    release: bool,
) -> Result<(), String> {
    let repo_root = find_repo_root(
        runner
            .parent()
            .ok_or_else(|| "runner path missing parent".to_string())?,
    )?;
    let (target_dir, rustc_tmpdir, cargo_incremental) = prepare_toolchain_env(&repo_root)?;
    let profile = if release { "release" } else { "debug" };
    let deps_dir = target_dir.join(profile).join("deps");
    super::emit_aot_build_progress(5, "build link dependencies");
    let (fusec_rlib, bincode_rlib, native_link_searches) = ensure_link_dependencies(
        &target_dir,
        &rustc_tmpdir,
        &cargo_incremental,
        profile,
        release,
        &deps_dir,
    )?;
    let selected_deps_dir = fusec_rlib
        .parent()
        .ok_or_else(|| format!("invalid fusec rlib path: {}", fusec_rlib.display()))?;
    super::emit_aot_build_progress(6, "link final binary");
    let mut rustc_cmd = ProcessCommand::new("rustc");
    apply_toolchain_env(
        &mut rustc_cmd,
        &target_dir,
        &rustc_tmpdir,
        &cargo_incremental,
    );
    rustc_cmd
        .arg("--edition=2024")
        .arg(runner)
        .arg("-o")
        .arg(out_path)
        .arg("-L")
        .arg(format!("dependency={}", selected_deps_dir.display()))
        .arg("--extern")
        .arg(format!("fusec={}", fusec_rlib.display()))
        .arg("--extern")
        .arg(format!("bincode={}", bincode_rlib.display()))
        .arg("-C")
        .arg(format!("link-arg={}", object.display()));
    for search in native_link_searches {
        rustc_cmd.arg("-L").arg(search);
    }
    if release {
        rustc_cmd.arg("-C").arg("opt-level=3");
    }
    let rustc_output = rustc_cmd
        .output()
        .map_err(|err| format!("failed to run rustc for native link: {err}"))?;
    if !rustc_output.status.success() {
        return Err(format!(
            "native link: rustc failed\n{}",
            summarize_command_failure(&rustc_output)
        ));
    }
    Ok(())
}

fn ensure_link_dependencies(
    target_dir: &Path,
    rustc_tmpdir: &Path,
    cargo_incremental: &str,
    profile: &str,
    release: bool,
    deps_dir: &Path,
) -> Result<(PathBuf, PathBuf, Vec<String>), String> {
    if let Some(existing) = resolve_usable_link_dependencies(
        target_dir,
        profile,
        deps_dir,
        rustc_tmpdir,
        cargo_incremental,
    ) {
        return Ok(existing);
    }

    // If stale release artifacts exist but are not linkable, prefer a known-good
    // debug dependency set before attempting a fresh release dependency build.
    // This keeps AOT release linking robust on hosts that cannot reliably
    // produce release metadata artifacts.
    if release && resolve_link_dependencies(target_dir, profile, deps_dir).is_some() {
        let fallback_deps = target_dir.join("debug").join("deps");
        if let Some(existing) = resolve_usable_link_dependencies(
            target_dir,
            "debug",
            &fallback_deps,
            rustc_tmpdir,
            cargo_incremental,
        ) {
            return Ok(existing);
        }
    }

    let primary_build_err =
        build_link_dependencies_for_profile(target_dir, rustc_tmpdir, cargo_incremental, release)
            .err();

    if let Some(existing) = resolve_usable_link_dependencies(
        target_dir,
        profile,
        deps_dir,
        rustc_tmpdir,
        cargo_incremental,
    ) {
        return Ok(existing);
    }

    if release {
        let fallback_deps = target_dir.join("debug").join("deps");
        if let Some(existing) = resolve_usable_link_dependencies(
            target_dir,
            "debug",
            &fallback_deps,
            rustc_tmpdir,
            cargo_incremental,
        ) {
            return Ok(existing);
        }
        let fallback_build_err =
            build_link_dependencies_for_profile(target_dir, rustc_tmpdir, cargo_incremental, false)
                .err();
        if let Some(existing) = resolve_usable_link_dependencies(
            target_dir,
            "debug",
            &fallback_deps,
            rustc_tmpdir,
            cargo_incremental,
        ) {
            return Ok(existing);
        }
        let mut reasons = Vec::new();
        if let Some(err) = primary_build_err {
            reasons.push(format!("release deps build failed: {err}"));
        } else {
            reasons.push("release deps unusable after build".to_string());
        }
        if let Some(err) = fallback_build_err {
            reasons.push(format!("debug fallback build failed: {err}"));
        } else {
            reasons.push("debug fallback deps unusable".to_string());
        }
        return Err(format!("native link: {}", reasons.join("; ")));
    }

    if let Some(err) = primary_build_err {
        return Err(format!(
            "native link: failed to build link dependencies\n{err}"
        ));
    }

    resolve_usable_link_dependencies(
        target_dir,
        profile,
        deps_dir,
        rustc_tmpdir,
        cargo_incremental,
    )
    .ok_or_else(|| {
        format!(
            "native link: missing link dependencies in {} after successful cargo build",
            deps_dir.display()
        )
    })
}

fn build_link_dependencies_for_profile(
    target_dir: &Path,
    rustc_tmpdir: &Path,
    cargo_incremental: &str,
    release: bool,
) -> Result<(), String> {
    let mut build_cmd = ProcessCommand::new("cargo");
    apply_toolchain_env(&mut build_cmd, target_dir, rustc_tmpdir, cargo_incremental);
    build_cmd.arg("build").arg("-p").arg("fusec");
    if release {
        build_cmd.arg("--release");
    }
    if env::var_os("CARGO_BUILD_JOBS").is_none() {
        build_cmd.env("CARGO_BUILD_JOBS", "1");
    }
    if env::var_os("CARGO_BUILD_PIPELINING").is_none() {
        build_cmd.env("CARGO_BUILD_PIPELINING", "false");
    }
    let build_output = build_cmd
        .output()
        .map_err(|err| format!("failed to run cargo build for link dependencies: {err}"))?;
    if build_output.status.success() {
        Ok(())
    } else {
        Err(summarize_command_failure(&build_output))
    }
}

fn resolve_usable_link_dependencies(
    target_dir: &Path,
    profile: &str,
    deps_dir: &Path,
    rustc_tmpdir: &Path,
    cargo_incremental: &str,
) -> Option<(PathBuf, PathBuf, Vec<String>)> {
    let resolved = resolve_link_dependencies(target_dir, profile, deps_dir)?;
    if probe_link_dependencies(
        target_dir,
        rustc_tmpdir,
        cargo_incremental,
        deps_dir,
        &resolved.0,
        &resolved.1,
        &resolved.2,
    )
    .is_ok()
    {
        Some(resolved)
    } else {
        None
    }
}

fn resolve_link_dependencies(
    target_dir: &Path,
    profile: &str,
    deps_dir: &Path,
) -> Option<(PathBuf, PathBuf, Vec<String>)> {
    let fusec_rlib = find_latest_rlib(deps_dir, "libfusec").ok()?;
    let bincode_rlib = find_latest_rlib(deps_dir, "libbincode").ok()?;
    let link_searches = collect_rustc_link_search_paths(target_dir, profile).ok()?;
    Some((fusec_rlib, bincode_rlib, link_searches))
}

fn probe_link_dependencies(
    target_dir: &Path,
    rustc_tmpdir: &Path,
    cargo_incremental: &str,
    deps_dir: &Path,
    fusec_rlib: &Path,
    bincode_rlib: &Path,
    native_link_searches: &[String],
) -> Result<(), String> {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let probe_name = format!("fuse_aot_link_probe_{stamp}_{pid}");
    let probe_src = rustc_tmpdir.join(format!("{probe_name}.rs"));
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    let probe_bin = rustc_tmpdir.join(format!("{probe_name}{exe_suffix}"));
    let probe_source = r#"fn main() {
    let _ = fusec::native::CACHE_VERSION;
    let _ = bincode::serialize(&0u8);
}
"#;
    fs::write(&probe_src, probe_source)
        .map_err(|err| format!("failed to write {}: {err}", probe_src.display()))?;

    let mut rustc_cmd = ProcessCommand::new("rustc");
    apply_toolchain_env(&mut rustc_cmd, target_dir, rustc_tmpdir, cargo_incremental);
    rustc_cmd
        .arg("--edition=2024")
        .arg(&probe_src)
        .arg("-o")
        .arg(&probe_bin)
        .arg("-L")
        .arg(format!("dependency={}", deps_dir.display()))
        .arg("--extern")
        .arg(format!("fusec={}", fusec_rlib.display()))
        .arg("--extern")
        .arg(format!("bincode={}", bincode_rlib.display()));
    for search in native_link_searches {
        rustc_cmd.arg("-L").arg(search);
    }
    let output = rustc_cmd
        .output()
        .map_err(|err| format!("failed to run rustc link dependency probe: {err}"))?;

    let _ = fs::remove_file(&probe_src);
    let _ = fs::remove_file(&probe_bin);

    if output.status.success() {
        Ok(())
    } else {
        Err(summarize_command_failure(&output))
    }
}

fn summarize_command_failure(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    "command failed without output".to_string()
}

fn prepare_toolchain_env(repo_root: &Path) -> Result<(PathBuf, PathBuf, String), String> {
    let target_dir = resolve_env_path_or_default(
        repo_root,
        "CARGO_TARGET_DIR",
        repo_root.join("tmp").join("fuse-target"),
    );
    let rustc_tmpdir =
        resolve_env_path_or_default(repo_root, "RUSTC_TMPDIR", target_dir.join("tmp"));
    fs::create_dir_all(&target_dir)
        .map_err(|err| format!("failed to create {}: {err}", target_dir.display()))?;
    fs::create_dir_all(&rustc_tmpdir)
        .map_err(|err| format!("failed to create {}: {err}", rustc_tmpdir.display()))?;
    let cargo_incremental = env::var("CARGO_INCREMENTAL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "0".to_string());
    Ok((target_dir, rustc_tmpdir, cargo_incremental))
}

fn resolve_env_path_or_default(repo_root: &Path, key: &str, default: PathBuf) -> PathBuf {
    let path = env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default);
    if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    }
}

fn apply_toolchain_env(
    cmd: &mut ProcessCommand,
    target_dir: &Path,
    rustc_tmpdir: &Path,
    cargo_incremental: &str,
) {
    cmd.env("CARGO_TARGET_DIR", target_dir);
    cmd.env("RUSTC_TMPDIR", rustc_tmpdir);
    cmd.env("TMPDIR", rustc_tmpdir);
    cmd.env("TMP", rustc_tmpdir);
    cmd.env("TEMP", rustc_tmpdir);
    cmd.env("CARGO_INCREMENTAL", cargo_incremental);
}

fn collect_rustc_link_search_paths(
    target_dir: &Path,
    profile: &str,
) -> Result<Vec<String>, String> {
    let build_dir = target_dir.join(profile).join("build");
    if !build_dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&build_dir)
        .map_err(|err| format!("failed to read {}: {err}", build_dir.display()))?;
    let mut searches = BTreeSet::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| format!("failed to read {}: {err}", build_dir.display()))?;
        let output_path = entry.path().join("output");
        if !output_path.exists() {
            continue;
        }
        let contents = fs::read_to_string(&output_path)
            .map_err(|err| format!("failed to read {}: {err}", output_path.display()))?;
        for line in contents.lines() {
            let Some(search) = line.strip_prefix("cargo:rustc-link-search=") else {
                continue;
            };
            let search = search.trim();
            if search.is_empty() {
                continue;
            }
            searches.insert(search.to_string());
        }
    }
    Ok(searches.into_iter().collect())
}

fn find_repo_root(start: &Path) -> Result<PathBuf, String> {
    let mut current = Some(start);
    while let Some(path) = current {
        let candidate = path.join("scripts").join("cargo_env.sh");
        if candidate.exists() {
            return Ok(path.to_path_buf());
        }
        current = path.parent();
    }
    Err("failed to locate repo root".to_string())
}

fn find_latest_rlib(dir: &Path, prefix: &str) -> Result<PathBuf, String> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let entries =
        fs::read_dir(dir).map_err(|err| format!("failed to read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("failed to read {}: {err}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rlib") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !file_name.starts_with(prefix) {
            continue;
        }
        let meta = entry
            .metadata()
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        match best {
            Some((best_time, _)) if modified <= best_time => {}
            _ => best = Some((modified, path)),
        }
    }
    best.map(|(_, path)| path)
        .ok_or_else(|| format!("failed to find {prefix}*.rlib in {}", dir.display()))
}

pub(crate) fn compile_artifacts(
    entry: &Path,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    strict_architecture: bool,
) -> Result<BuildArtifacts, String> {
    let src = fs::read_to_string(entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        super::emit_diags(&diags);
        return Err("build failed".to_string());
    }
    let (_analysis, sema_diags) = fusec::sema::analyze_registry_with_options(
        &registry,
        fusec::sema::AnalyzeOptions {
            strict_architecture,
        },
    );
    if !sema_diags.is_empty() {
        super::emit_diags(&sema_diags);
        return Err("build failed".to_string());
    }
    let ir = fusec::ir::lower::lower_registry(&registry).map_err(|errors| {
        let mut out = String::new();
        for err in errors {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("lowering error: {err}"));
        }
        out
    })?;
    let native = fusec::native::NativeProgram::from_ir(ir);
    let meta = super::build_ir_meta(&registry, manifest_dir)?;
    Ok(BuildArtifacts { native, meta })
}

pub(crate) fn write_compiled_artifacts(
    manifest_dir: Option<&Path>,
    artifacts: &BuildArtifacts,
) -> Result<(), String> {
    let build_dir = super::build_dir(manifest_dir)?;
    if !build_dir.exists() {
        fs::create_dir_all(&build_dir)
            .map_err(|err| format!("failed to create {}: {err}", build_dir.display()))?;
    }

    let native_path = build_dir.join("program.native");
    let native_bytes = bincode::serialize(&artifacts.native)
        .map_err(|err| format!("native encode failed: {err}"))?;
    fs::write(&native_path, native_bytes)
        .map_err(|err| format!("failed to write {}: {err}", native_path.display()))?;

    let meta_path = build_dir.join("program.meta");
    super::write_ir_meta(&meta_path, &artifacts.meta)?;
    Ok(())
}

pub(crate) fn try_load_native(manifest_dir: Option<&Path>) -> Option<fusec::native::NativeProgram> {
    let build_dir = super::build_dir(manifest_dir).ok()?;
    let path = build_dir.join("program.native");
    let meta_path = build_dir.join("program.meta");
    let meta = super::load_ir_meta(&meta_path)?;
    if !super::ir_meta_is_valid(&meta, manifest_dir) {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let native: fusec::native::NativeProgram = bincode::deserialize(&bytes).ok()?;
    if native.version != fusec::native::NativeProgram::VERSION {
        return None;
    }
    Some(native)
}

pub(crate) fn run_native_program(
    program: fusec::native::NativeProgram,
    app: Option<&str>,
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
    program_args: &[String],
) -> i32 {
    let mut vm = fusec::native::NativeVm::new(&program);
    if program_args.is_empty() {
        return match vm.run_app(app) {
            Ok(_) => 0,
            Err(err) => {
                if diagnostics_json_enabled() {
                    emit_validation_error("$", classify_runtime_error_code(&err), &err);
                } else {
                    super::emit_cli_error(&format!("run error: {err}"));
                }
                1
            }
        };
    }
    let (entry_name, args) = match prepare_cached_cli_call(entry, deps, program_args) {
        Ok(value) => value,
        Err(code) => return code,
    };
    match vm.call_function(&entry_name, args) {
        Ok(_) => 0,
        Err(err) => {
            emit_error_json_message(&err);
            2
        }
    }
}

fn prepare_cached_cli_call(
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
    program_args: &[String],
) -> Result<(String, Vec<fusec::interp::Value>), i32> {
    let src = match fs::read_to_string(entry) {
        Ok(src) => src,
        Err(err) => {
            super::emit_cli_error(&format!("failed to read {}: {err}", entry.display()));
            return Err(1);
        }
    };
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        super::emit_diags_with_fallback(&diags, Some((entry, &src)));
        return Err(1);
    }
    let root = match registry.root() {
        Some(root) => root,
        None => {
            super::emit_cli_error("no root module loaded");
            return Err(1);
        }
    };
    let main_decl = root.program.items.iter().find_map(|item| match item {
        fusec::ast::Item::Fn(decl) if decl.name.name == "main" => Some(decl),
        _ => None,
    });
    let Some(main_decl) = main_decl else {
        super::emit_cli_error("no root fn main found for CLI binding");
        return Err(1);
    };

    let raw = match parse_program_args(program_args) {
        Ok(raw) => raw,
        Err(err) => {
            emit_validation_error("$", "invalid_args", &err);
            return Err(2);
        }
    };

    let mut interp = fusec::interp::Interpreter::with_registry(&registry);
    let mut args_map = HashMap::new();
    let mut errors = Vec::new();
    let param_names: HashSet<String> = main_decl
        .params
        .iter()
        .map(|param| param.name.name.clone())
        .collect();
    for (name, _) in &raw.values {
        if !param_names.contains(name) {
            errors.push(ValidationField {
                path: name.clone(),
                code: "unknown_flag".to_string(),
                message: "unknown flag".to_string(),
            });
        }
    }
    for (name, _) in &raw.bools {
        if !param_names.contains(name) {
            errors.push(ValidationField {
                path: name.clone(),
                code: "unknown_flag".to_string(),
                message: "unknown flag".to_string(),
            });
        }
    }
    for param in &main_decl.params {
        let name = &param.name.name;
        if let Some(flag) = raw.bools.get(name) {
            if !is_bool_type(&param.ty) {
                errors.push(ValidationField {
                    path: name.clone(),
                    code: "invalid_type".to_string(),
                    message: "expected Bool flag".to_string(),
                });
                continue;
            }
            args_map.insert(name.clone(), fusec::interp::Value::Bool(*flag));
            continue;
        }
        if let Some(values) = raw.values.get(name) {
            if values.len() != 1 {
                errors.push(ValidationField {
                    path: name.clone(),
                    code: "invalid_type".to_string(),
                    message: "multiple values not supported".to_string(),
                });
                continue;
            }
            match interp.parse_cli_value(&param.ty, &values[0]) {
                Ok(value) => {
                    args_map.insert(name.clone(), value);
                }
                Err(msg) => {
                    errors.push(ValidationField {
                        path: name.clone(),
                        code: "invalid_value".to_string(),
                        message: msg,
                    });
                }
            }
            continue;
        }
        if param.default.is_none() && !is_optional(&param.ty) {
            errors.push(ValidationField {
                path: name.clone(),
                code: "missing_field".to_string(),
                message: "missing flag".to_string(),
            });
        }
    }
    if !errors.is_empty() {
        emit_validation_error_fields(errors);
        return Err(2);
    }
    let args = match interp.prepare_call_with_named_args("main", &args_map) {
        Ok(args) => args,
        Err(err) => {
            emit_error_json_message(&err);
            return Err(2);
        }
    };
    let entry_name = fusec::ir::lower::canonical_function_name(registry.root, "main");
    Ok((entry_name, args))
}

fn parse_program_args(args: &[String]) -> Result<RawProgramArgs, String> {
    let mut values: HashMap<String, Vec<String>> = HashMap::new();
    let mut bools: HashMap<String, bool> = HashMap::new();
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = &args[idx];
        if !arg.starts_with("--") {
            return Err(format!("unexpected argument: {arg}"));
        }
        if let Some((name, val)) = arg.strip_prefix("--").and_then(|s| s.split_once('=')) {
            values
                .entry(name.to_string())
                .or_default()
                .push(val.to_string());
            idx += 1;
            continue;
        }
        if let Some(name) = arg.strip_prefix("--no-") {
            bools.insert(name.to_string(), false);
            idx += 1;
            continue;
        }
        let name = arg.trim_start_matches("--");
        if idx + 1 < args.len() && !args[idx + 1].starts_with("--") {
            values
                .entry(name.to_string())
                .or_default()
                .push(args[idx + 1].clone());
            idx += 2;
        } else {
            bools.insert(name.to_string(), true);
            idx += 1;
        }
    }
    Ok(RawProgramArgs { values, bools })
}

fn is_optional(ty: &fusec::ast::TypeRef) -> bool {
    match &ty.kind {
        fusec::ast::TypeRefKind::Optional(_) => true,
        fusec::ast::TypeRefKind::Generic { base, .. } => base.name == "Option",
        _ => false,
    }
}

fn is_bool_type(ty: &fusec::ast::TypeRef) -> bool {
    match &ty.kind {
        fusec::ast::TypeRefKind::Simple(ident) => ident.name == "Bool",
        fusec::ast::TypeRefKind::Refined { base, .. } => base.name == "Bool",
        fusec::ast::TypeRefKind::Optional(inner) => is_bool_type(inner),
        fusec::ast::TypeRefKind::Generic { base, args } => {
            if base.name == "Option" && args.len() == 1 {
                is_bool_type(&args[0])
            } else {
                false
            }
        }
        _ => false,
    }
}

fn emit_validation_error(path: &str, code: &str, message: &str) {
    emit_validation_error_fields(vec![ValidationField {
        path: path.to_string(),
        code: code.to_string(),
        message: message.to_string(),
    }]);
}

fn emit_validation_error_fields(fields: Vec<ValidationField>) {
    let err = ValidationError {
        message: "validation failed".to_string(),
        fields,
    };
    eprintln!("{}", rt_json::encode(&err.to_json()));
}

fn emit_error_json_message(message: &str) {
    if message.trim_start().starts_with('{') {
        eprintln!("{message}");
    } else {
        emit_validation_error("$", classify_runtime_error_code(message), message);
    }
}

fn classify_runtime_error_code(message: &str) -> &'static str {
    let message = message.trim();
    if message == "null access" {
        return "runtime_null_access";
    }
    if message == "index out of bounds" {
        return "runtime_index_bounds";
    }
    if message == "list index must be Int" || message == "map keys must be strings" {
        return "runtime_invalid_index";
    }
    if message.starts_with("field lookup failed:") {
        return "runtime_field_access";
    }
    if message == "assignment target must be an indexable value"
        || message == "assignment target must be a struct field"
    {
        return "runtime_invalid_assignment_target";
    }
    if message == "await expects a Task value" {
        return "runtime_task_expected";
    }
    if message.starts_with("range expects")
        || message == "range start must be <= end"
        || message == "invalid range bounds"
    {
        return "runtime_range_error";
    }
    if message.starts_with("unsupported comparison")
        || message == "unsupported + operands"
        || message == "cannot iterate over value"
        || message.starts_with("expected iterator, got")
    {
        return "runtime_type_error";
    }
    if message.contains(" expects ") || message.starts_with("expected ") {
        return "runtime_invalid_arguments";
    }
    "runtime_error"
}
