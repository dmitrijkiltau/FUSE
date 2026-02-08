use std::collections::{BTreeMap, HashMap, VecDeque};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

const USAGE: &str = r#"usage: fuse <command> [options] [file] [-- <program args>]

commands:
  run       Run the package entrypoint
  test      Run tests in the package
  build     Run package checks (and optional build steps)
  check     Parse + sema check
  fmt       Format a Fuse file
  openapi   Emit OpenAPI JSON
  migrate   Run database migrations

options:
  --manifest-path <path>  Path to fuse.toml (defaults to nearest parent)
  --file <path>           Entry file override
  --app <name>            App name override
  --backend <ast|vm|native>  Backend override (run only)
  --clean                 Remove .fuse/build before building (build only)
"#;

#[derive(Debug, Deserialize)]
struct Manifest {
    package: PackageConfig,
    #[serde(default)]
    build: Option<BuildConfig>,
    #[serde(default)]
    serve: Option<ServeConfig>,
    #[serde(default)]
    dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, Deserialize)]
struct PackageConfig {
    #[serde(alias = "main")]
    entry: Option<String>,
    app: Option<String>,
    backend: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BuildConfig {
    openapi: Option<String>,
    native_bin: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServeConfig {
    static_dir: Option<String>,
    static_index: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum DependencySpec {
    Simple(String),
    Detailed(DependencyDetail),
}

#[derive(Debug, Deserialize, Clone, Default)]
struct DependencyDetail {
    version: Option<String>,
    path: Option<String>,
    git: Option<String>,
    rev: Option<String>,
    tag: Option<String>,
    branch: Option<String>,
    subdir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Lockfile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    dependencies: BTreeMap<String, LockedDependency>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct LockedDependency {
    source: String,
    git: Option<String>,
    rev: Option<String>,
    path: Option<String>,
    subdir: Option<String>,
    requested: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct IrMeta {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    native_cache_version: u32,
    #[serde(default)]
    files: Vec<IrFileMeta>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IrFileMeta {
    path: String,
    modified_secs: u64,
    modified_nanos: u32,
    size: u64,
}

struct BuildArtifacts {
    ir: fusec::ir::Program,
    native: fusec::native::NativeProgram,
    meta: IrMeta,
}

#[derive(Default)]
struct CommonArgs {
    manifest_path: Option<PathBuf>,
    entry: Option<String>,
    app: Option<String>,
    backend: Option<String>,
    program_args: Vec<String>,
    clean: bool,
}

enum Command {
    Run,
    Test,
    Build,
    Check,
    Fmt,
    Openapi,
    Migrate,
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum RunBackend {
    Ast,
    Vm,
    Native,
}

impl RunBackend {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "ast" => Some(Self::Ast),
            "vm" => Some(Self::Vm),
            "native" => Some(Self::Native),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ast => "ast",
            Self::Vm => "vm",
            Self::Native => "native",
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let code = run(args);
    std::process::exit(code);
}

fn run(args: Vec<String>) -> i32 {
    if args.is_empty() {
        eprintln!("{USAGE}");
        return 1;
    }
    let (cmd, rest) = args.split_first().unwrap();
    let command = match cmd.as_str() {
        "run" => Command::Run,
        "test" => Command::Test,
        "build" => Command::Build,
        "check" => Command::Check,
        "fmt" => Command::Fmt,
        "openapi" => Command::Openapi,
        "migrate" => Command::Migrate,
        _ => {
            eprintln!("unknown command: {cmd}");
            eprintln!("{USAGE}");
            return 1;
        }
    };
    let allow_program_args = matches!(command, Command::Run);
    let allow_clean = matches!(command, Command::Build);
    let common = match parse_common_args(rest, allow_program_args, allow_clean) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("{err}");
            eprintln!("{USAGE}");
            return 1;
        }
    };

    let (manifest, manifest_dir) = match load_manifest(common.manifest_path.as_deref()) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    apply_dotenv(manifest_dir.as_deref());
    apply_default_config_path(manifest_dir.as_deref());

    let entry = resolve_entry(&common, manifest.as_ref(), manifest_dir.as_deref());
    let entry = match entry {
        Ok(entry) => entry,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    let app = common.app.clone().or_else(|| {
        if common.entry.is_some() {
            None
        } else {
            manifest.as_ref().and_then(|m| m.package.app.clone())
        }
    });
    let backend = common
        .backend
        .clone()
        .or_else(|| manifest.as_ref().and_then(|m| m.package.backend.clone()));
    let backend = if let Some(name) = backend {
        match RunBackend::parse(&name) {
            Some(backend) => Some(backend),
            None => {
                eprintln!("unknown backend: {name}");
                return 1;
            }
        }
    } else {
        None
    };

    match command {
        Command::Run if common.program_args.is_empty() => match backend {
            Some(RunBackend::Ast) => {}
            Some(RunBackend::Native) => {
                if let Some(native) = try_load_native(manifest_dir.as_deref()) {
                    apply_serve_env(manifest.as_ref(), manifest_dir.as_deref());
                    return run_native_program(native, app.as_deref());
                }
            }
            _ => {
                if let Some(ir) = try_load_ir(manifest_dir.as_deref()) {
                    apply_serve_env(manifest.as_ref(), manifest_dir.as_deref());
                    return run_vm_ir(ir, app.as_deref());
                }
            }
        },
        _ => {}
    }

    let backend_flag = backend.map(|backend| backend.as_str().to_string());

    let deps = match resolve_dependencies(manifest.as_ref(), manifest_dir.as_deref()) {
        Ok(deps) => deps,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    match command {
        Command::Run => {
            apply_serve_env(manifest.as_ref(), manifest_dir.as_deref());
            let mut args = Vec::new();
            args.push("--run".to_string());
            if let Some(backend) = backend_flag {
                args.push("--backend".to_string());
                args.push(backend);
            }
            if let Some(app) = app {
                args.push("--app".to_string());
                args.push(app);
            }
            args.push(entry.to_string_lossy().to_string());
            if !common.program_args.is_empty() {
                args.push("--".to_string());
                args.extend(common.program_args);
            }
            fusec::cli::run_with_deps(args, Some(&deps))
        }
        Command::Test => {
            let mut args = Vec::new();
            args.push("--test".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
        Command::Build => run_build(
            &entry,
            manifest.as_ref(),
            manifest_dir.as_deref(),
            &deps,
            app.as_deref(),
            common.clean,
        ),
        Command::Check => {
            let mut args = Vec::new();
            args.push("--check".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
        Command::Fmt => {
            let mut args = Vec::new();
            args.push("--fmt".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
        Command::Openapi => {
            let mut args = Vec::new();
            args.push("--openapi".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
        Command::Migrate => {
            let mut args = Vec::new();
            args.push("--migrate".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
    }
}

fn apply_serve_env(manifest: Option<&Manifest>, manifest_dir: Option<&Path>) {
    let Some(serve) = manifest.and_then(|m| m.serve.as_ref()) else {
        return;
    };
    let Some(static_dir) = serve.static_dir.as_ref() else {
        return;
    };
    let mut resolved = PathBuf::from(static_dir);
    if resolved.is_relative() {
        if let Some(base) = manifest_dir {
            resolved = base.join(resolved);
        }
    }
    unsafe {
        env::set_var("FUSE_STATIC_DIR", resolved.to_string_lossy().to_string());
        if let Some(index) = serve.static_index.as_ref() {
            env::set_var("FUSE_STATIC_INDEX", index);
        }
    }
}

fn apply_dotenv(manifest_dir: Option<&Path>) {
    let mut path = PathBuf::from(".env");
    if let Some(dir) = manifest_dir {
        path = dir.join(".env");
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            eprintln!("failed to read {}: {err}", path.display());
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

fn apply_default_config_path(manifest_dir: Option<&Path>) {
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

fn parse_common_args(
    args: &[String],
    allow_program_args: bool,
    allow_clean: bool,
) -> Result<CommonArgs, String> {
    let mut out = CommonArgs::default();
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            if allow_program_args {
                out.program_args.extend(args[idx + 1..].iter().cloned());
                break;
            } else {
                return Err("unexpected --".to_string());
            }
        }
        if arg == "--manifest-path" {
            idx += 1;
            let Some(path) = args.get(idx) else {
                return Err("--manifest-path expects a path".to_string());
            };
            out.manifest_path = Some(PathBuf::from(path));
            idx += 1;
            continue;
        }
        if arg == "--file" {
            idx += 1;
            let Some(path) = args.get(idx) else {
                return Err("--file expects a path".to_string());
            };
            out.entry = Some(path.clone());
            idx += 1;
            continue;
        }
        if arg == "--app" {
            idx += 1;
            let Some(name) = args.get(idx) else {
                return Err("--app expects a name".to_string());
            };
            out.app = Some(name.clone());
            idx += 1;
            continue;
        }
        if arg == "--backend" {
            idx += 1;
            let Some(name) = args.get(idx) else {
                return Err("--backend expects a name".to_string());
            };
            out.backend = Some(name.clone());
            idx += 1;
            continue;
        }
        if arg == "--clean" {
            if !allow_clean {
                return Err("--clean is only supported for fuse build".to_string());
            }
            out.clean = true;
            idx += 1;
            continue;
        }
        if arg.starts_with("--") {
            return Err(format!("unknown option: {arg}"));
        }
        if out.entry.is_none() {
            if out.manifest_path.is_none() {
                let candidate = PathBuf::from(arg);
                if candidate.is_dir() && candidate.join("fuse.toml").exists() {
                    out.manifest_path = Some(candidate);
                    idx += 1;
                    continue;
                }
            }
            out.entry = Some(arg.clone());
            idx += 1;
            continue;
        }
        return Err(format!("unexpected argument: {arg}"));
    }
    Ok(out)
}

fn load_manifest(
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

fn find_manifest(start: &Path) -> Option<PathBuf> {
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

fn resolve_entry(
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

fn run_build(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    app: Option<&str>,
    clean: bool,
) -> i32 {
    if clean {
        if let Err(err) = clean_build_dir(manifest_dir) {
            eprintln!("{err}");
            return 1;
        }
        return 0;
    }
    let mut check_args = Vec::new();
    check_args.push("--check".to_string());
    check_args.push(entry.to_string_lossy().to_string());
    let code = fusec::cli::run_with_deps(check_args, Some(deps));
    if code != 0 {
        return code;
    }
    let artifacts = match compile_artifacts(entry, deps) {
        Ok(artifacts) => artifacts,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if let Err(err) = write_compiled_artifacts(manifest_dir, &artifacts) {
        eprintln!("{err}");
        return 1;
    }
    if let Some(native_bin) = manifest.and_then(|m| m.build.as_ref().and_then(|b| b.native_bin.clone())) {
        if let Err(err) = write_native_binary(manifest_dir, &artifacts.native, app, &native_bin) {
            eprintln!("{err}");
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
                    eprintln!("cwd error: {err}");
                    return 1;
                }
            }
        }
    };
    if let Err(err) = write_openapi(entry, &out_path, deps) {
        eprintln!("{err}");
        return 1;
    }
    0
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
        for diag in diags {
            let level = match diag.level {
                fusec::diag::Level::Error => "error",
                fusec::diag::Level::Warning => "warning",
            };
            eprintln!(
                "{level}: {} ({}..{})",
                diag.message, diag.span.start, diag.span.end
            );
        }
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

fn write_native_binary(
    manifest_dir: Option<&Path>,
    program: &fusec::native::NativeProgram,
    app: Option<&str>,
    out_path: &str,
) -> Result<(), String> {
    let build_dir = build_dir(manifest_dir)?;
    if !build_dir.exists() {
        fs::create_dir_all(&build_dir)
            .map_err(|err| format!("failed to create {}: {err}", build_dir.display()))?;
    }
    let artifact = fusec::native::emit_object_for_app(program, app)?;
    let object_path = build_dir.join("program.o");
    fs::write(&object_path, &artifact.object)
        .map_err(|err| format!("failed to write {}: {err}", object_path.display()))?;
    let mut configs: Vec<fusec::ir::Config> =
        program.ir.configs.values().cloned().collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    let config_bytes =
        bincode::serialize(&configs).map_err(|err| format!("config encode failed: {err}"))?;
    let mut types: Vec<fusec::ir::TypeInfo> =
        program.ir.types.values().cloned().collect();
    types.sort_by(|a, b| a.name.cmp(&b.name));
    let type_bytes =
        bincode::serialize(&types).map_err(|err| format!("type encode failed: {err}"))?;
    let runner_path = build_dir.join("native_main.rs");
    write_native_runner(
        &runner_path,
        &artifact.entry_symbol,
        &artifact.interned_strings,
        &config_bytes,
        &type_bytes,
        &artifact.config_defaults,
    )?;
    let out_path = resolve_output_path(manifest_dir, out_path)?;
    if let Some(parent) = out_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
    }
    link_native_binary(&runner_path, &object_path, &out_path)?;
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
    config_bytes: &[u8],
    type_bytes: &[u8],
    config_defaults: &[fusec::native::ConfigDefaultSymbol],
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
    let source = format!(
        r#"use fusec::interp::format_error_value;
use fusec::native::value::{{NativeHeap, NativeValue}};
use fusec::native::{{load_configs_for_binary, load_types_for_binary}};

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
const CONFIG_BYTES: &[u8] = {config_blob};
const TYPE_BYTES: &[u8] = {type_blob};

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

fn load_configs(heap: &mut NativeHeap) -> Result<(), String> {{
    if CONFIG_BYTES.is_empty() {{
        return Ok(());
    }}
    let configs: Vec<fusec::ir::Config> =
        bincode::deserialize(CONFIG_BYTES).map_err(|err| format!("config decode failed: {{err}}"))?;
    load_configs_for_binary(configs.iter(), heap, |name, heap| call_default(name, heap))
}}

fn load_types(heap: &mut NativeHeap) -> Result<(), String> {{
    if TYPE_BYTES.is_empty() {{
        return Ok(());
    }}
    let types: Vec<fusec::ir::TypeInfo> =
        bincode::deserialize(TYPE_BYTES).map_err(|err| format!("type decode failed: {{err}}"))?;
    load_types_for_binary(types.iter(), heap)
}}

fn main() {{
    let mut heap = NativeHeap::new();
    for value in INTERNED_STRINGS {{
        heap.intern_string((*value).to_string());
    }}
    if let Err(err) = load_types(&mut heap) {{
        eprintln!("run error: {{err}}");
        std::process::exit(1);
    }}
    if let Err(err) = load_configs(&mut heap) {{
        eprintln!("run error: {{err}}");
        std::process::exit(1);
    }}
    if let Err(err) = call_native(fuse_entry, &mut heap) {{
        eprintln!("run error: {{err}}");
        std::process::exit(1);
    }}
}}
"#
    );
    fs::write(path, source)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(())
}

fn link_native_binary(runner: &Path, object: &Path, out_path: &Path) -> Result<(), String> {
    let repo_root = find_repo_root(
        runner
            .parent()
            .ok_or_else(|| "runner path missing parent".to_string())?,
    )?;
    let script = repo_root.join("scripts").join("cargo_env.sh");
    let status = ProcessCommand::new(&script)
        .arg("cargo")
        .arg("build")
        .arg("-p")
        .arg("fusec")
        .status()
        .map_err(|err| format!("failed to run {}: {err}", script.display()))?;
    if !status.success() {
        return Err("native link: failed to build fusec".to_string());
    }
    let target_dir = match env::var("CARGO_TARGET_DIR") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => repo_root.join("tmp").join("fuse-target"),
    };
    let deps_dir = target_dir.join("debug").join("deps");
    let fusec_rlib = find_latest_rlib(&deps_dir, "libfusec")?;
    let bincode_rlib = find_latest_rlib(&deps_dir, "libbincode")?;
    let status = ProcessCommand::new(&script)
        .arg("rustc")
        .arg("--edition=2024")
        .arg(runner)
        .arg("-o")
        .arg(out_path)
        .arg("-L")
        .arg(format!("dependency={}", deps_dir.display()))
        .arg("--extern")
        .arg(format!("fusec={}", fusec_rlib.display()))
        .arg("--extern")
        .arg(format!("bincode={}", bincode_rlib.display()))
        .arg("-C")
        .arg(format!("link-arg={}", object.display()))
        .status()
        .map_err(|err| format!("failed to run rustc via {}: {err}", script.display()))?;
    if !status.success() {
        return Err("native link: rustc failed".to_string());
    }
    Ok(())
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
    let entries = fs::read_dir(dir)
        .map_err(|err| format!("failed to read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("failed to read {}: {err}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rlib") {
            continue;
        }
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
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

fn compile_artifacts(
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
) -> Result<BuildArtifacts, String> {
    let src = fs::read_to_string(entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        for diag in diags {
            let level = match diag.level {
                fusec::diag::Level::Error => "error",
                fusec::diag::Level::Warning => "warning",
            };
            eprintln!(
                "{level}: {} ({}..{})",
                diag.message, diag.span.start, diag.span.end
            );
        }
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
    let native = fusec::native::NativeProgram::from_ir(ir.clone());
    let meta = build_ir_meta(&registry)?;
    Ok(BuildArtifacts { ir, native, meta })
}

fn write_compiled_artifacts(
    manifest_dir: Option<&Path>,
    artifacts: &BuildArtifacts,
) -> Result<(), String> {
    let build_dir = build_dir(manifest_dir)?;
    if !build_dir.exists() {
        fs::create_dir_all(&build_dir)
            .map_err(|err| format!("failed to create {}: {err}", build_dir.display()))?;
    }

    let ir_path = build_dir.join("program.ir");
    let ir_bytes =
        bincode::serialize(&artifacts.ir).map_err(|err| format!("ir encode failed: {err}"))?;
    fs::write(&ir_path, ir_bytes)
        .map_err(|err| format!("failed to write {}: {err}", ir_path.display()))?;

    let native_path = build_dir.join("program.native");
    let native_bytes = bincode::serialize(&artifacts.native)
        .map_err(|err| format!("native encode failed: {err}"))?;
    fs::write(&native_path, native_bytes)
        .map_err(|err| format!("failed to write {}: {err}", native_path.display()))?;

    let meta_path = build_dir.join("program.meta");
    let meta_bytes = bincode::serialize(&artifacts.meta)
        .map_err(|err| format!("ir meta encode failed: {err}"))?;
    fs::write(&meta_path, meta_bytes)
        .map_err(|err| format!("failed to write {}: {err}", meta_path.display()))?;
    Ok(())
}

fn try_load_ir(manifest_dir: Option<&Path>) -> Option<fusec::ir::Program> {
    let build_dir = build_dir(manifest_dir).ok()?;
    let path = build_dir.join("program.ir");
    let meta_path = build_dir.join("program.meta");
    let meta = load_ir_meta(&meta_path)?;
    if !ir_meta_is_valid(&meta) {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    bincode::deserialize(&bytes).ok()
}

fn try_load_native(manifest_dir: Option<&Path>) -> Option<fusec::native::NativeProgram> {
    let build_dir = build_dir(manifest_dir).ok()?;
    let path = build_dir.join("program.native");
    let meta_path = build_dir.join("program.meta");
    let meta = load_ir_meta(&meta_path)?;
    if !ir_meta_is_valid(&meta) {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let native: fusec::native::NativeProgram = bincode::deserialize(&bytes).ok()?;
    if native.version != fusec::native::NativeProgram::VERSION {
        return None;
    }
    Some(native)
}

fn run_vm_ir(ir: fusec::ir::Program, app: Option<&str>) -> i32 {
    let mut vm = fusec::vm::Vm::new(&ir);
    match vm.run_app(app) {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("run error: {err}");
            1
        }
    }
}

fn run_native_program(program: fusec::native::NativeProgram, app: Option<&str>) -> i32 {
    let mut vm = fusec::native::NativeVm::new(&program);
    match vm.run_app(app) {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("run error: {err}");
            1
        }
    }
}

fn build_dir(manifest_dir: Option<&Path>) -> Result<PathBuf, String> {
    let base = match manifest_dir {
        Some(dir) => dir.to_path_buf(),
        None => env::current_dir().map_err(|err| format!("cwd error: {err}"))?,
    };
    Ok(base.join(".fuse").join("build"))
}

fn clean_build_dir(manifest_dir: Option<&Path>) -> Result<(), String> {
    let dir = build_dir(manifest_dir)?;
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .map_err(|err| format!("failed to remove {}: {err}", dir.display()))?;
    }
    Ok(())
}

fn build_ir_meta(registry: &fusec::ModuleRegistry) -> Result<IrMeta, String> {
    let mut files = Vec::new();
    for unit in registry.modules.values() {
        let stamp = file_stamp(&unit.path)?;
        files.push(IrFileMeta {
            path: unit.path.to_string_lossy().to_string(),
            modified_secs: stamp.modified_secs,
            modified_nanos: stamp.modified_nanos,
            size: stamp.size,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(IrMeta {
        version: 2,
        native_cache_version: fusec::native::CACHE_VERSION,
        files,
    })
}

fn load_ir_meta(path: &Path) -> Option<IrMeta> {
    let bytes = fs::read(path).ok()?;
    bincode::deserialize(&bytes).ok()
}

fn ir_meta_is_valid(meta: &IrMeta) -> bool {
    if meta.version != 2 || meta.files.is_empty() {
        return false;
    }
    if meta.native_cache_version != fusec::native::CACHE_VERSION {
        return false;
    }
    for file in &meta.files {
        let stamp = match file_stamp(Path::new(&file.path)) {
            Ok(stamp) => stamp,
            Err(_) => return false,
        };
        if stamp.modified_secs != file.modified_secs
            || stamp.modified_nanos != file.modified_nanos
            || stamp.size != file.size
        {
            return false;
        }
    }
    true
}

struct FileStamp {
    modified_secs: u64,
    modified_nanos: u32,
    size: u64,
}

fn file_stamp(path: &Path) -> Result<FileStamp, String> {
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let modified = metadata
        .modified()
        .map_err(|err| format!("failed to read mtime for {}: {err}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("mtime before epoch for {}: {err}", path.display()))?;
    Ok(FileStamp {
        modified_secs: duration.as_secs(),
        modified_nanos: duration.subsec_nanos(),
        size: metadata.len(),
    })
}

fn resolve_dependencies(
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
) -> Result<HashMap<String, PathBuf>, String> {
    let Some(manifest) = manifest else {
        return Ok(HashMap::new());
    };
    if manifest.dependencies.is_empty() {
        return Ok(HashMap::new());
    }
    let Some(root_dir) = manifest_dir else {
        return Err("dependencies require a manifest directory".to_string());
    };
    let lock_path = root_dir.join("fuse.lock");
    let mut lock = load_lockfile(&lock_path)?;
    let deps_dir = root_dir.join(".fuse").join("deps");
    if !deps_dir.exists() {
        fs::create_dir_all(&deps_dir)
            .map_err(|err| format!("failed to create {}: {err}", deps_dir.display()))?;
    }

    let mut resolved = HashMap::new();
    let mut requests: HashMap<String, String> = HashMap::new();
    let mut queue: VecDeque<(String, DependencySpec, PathBuf)> = VecDeque::new();
    for (name, spec) in &manifest.dependencies {
        queue.push_back((name.clone(), spec.clone(), root_dir.to_path_buf()));
    }

    while let Some((name, spec, base_dir)) = queue.pop_front() {
        let requested = dependency_request_key(&spec, &base_dir)?;
        if let Some(prev) = requests.get(&name) {
            if prev != &requested {
                return Err(format!(
                    "dependency {name} requested with conflicting specs: {prev} vs {requested}"
                ));
            }
        } else {
            requests.insert(name.clone(), requested);
        }
        if resolved.contains_key(&name) {
            continue;
        }
        let root = resolve_dependency(&name, &spec, &base_dir, root_dir, &deps_dir, &mut lock)?;
        resolved.insert(name.clone(), root.clone());

        if let Some(dep_manifest) = load_manifest_from_dir(&root)? {
            for (dep_name, dep_spec) in dep_manifest.dependencies {
                queue.push_back((dep_name, dep_spec, root.clone()));
            }
        }
    }

    lock.version = 1;
    write_lockfile(&lock_path, &lock)?;
    Ok(resolved)
}

fn dependency_request_key(spec: &DependencySpec, base_dir: &Path) -> Result<String, String> {
    let normalized = normalize_dependency_spec(spec, base_dir)?;
    Ok(normalized.requested)
}

fn resolve_dependency(
    name: &str,
    spec: &DependencySpec,
    base_dir: &Path,
    root_dir: &Path,
    deps_dir: &Path,
    lock: &mut Lockfile,
) -> Result<PathBuf, String> {
    let normalized = normalize_dependency_spec(spec, base_dir)?;
    if let Some(entry) = lock.dependencies.get(name) {
        if entry.requested.as_deref() == Some(normalized.requested.as_str()) {
            return root_from_lock(name, entry, root_dir, deps_dir);
        }
    }

    let (root, entry) = match normalized.kind {
        NormalizedKind::Path { path } => {
            if !path.exists() {
                return Err(format!(
                    "dependency {name} path does not exist: {}",
                    path.display()
                ));
            }
            let stored_path = store_path(root_dir, &path);
            (
                path,
                LockedDependency {
                    source: "path".to_string(),
                    git: None,
                    rev: None,
                    path: Some(stored_path),
                    subdir: None,
                    requested: Some(normalized.requested),
                },
            )
        }
        NormalizedKind::Git { git, reference } => {
            let rev = resolve_git_revision(&git, &reference)?;
            let checkout = deps_dir.join(name).join(&rev);
            ensure_checkout(&git, &rev, &checkout)?;
            let root = if let Some(subdir) = &reference.subdir {
                checkout.join(subdir)
            } else {
                checkout
            };
            if !root.exists() {
                return Err(format!(
                    "dependency {name} subdir does not exist: {}",
                    root.display()
                ));
            }
            (
                root,
                LockedDependency {
                    source: "git".to_string(),
                    git: Some(git),
                    rev: Some(rev),
                    path: None,
                    subdir: reference.subdir,
                    requested: Some(normalized.requested),
                },
            )
        }
    };

    lock.dependencies.insert(name.to_string(), entry);
    Ok(root)
}

fn root_from_lock(
    name: &str,
    entry: &LockedDependency,
    root_dir: &Path,
    deps_dir: &Path,
) -> Result<PathBuf, String> {
    match entry.source.as_str() {
        "path" => {
            let Some(path) = &entry.path else {
                return Err(format!("lock entry for {name} missing path"));
            };
            let path = PathBuf::from(path);
            if path.is_absolute() {
                Ok(path)
            } else {
                Ok(root_dir.join(path))
            }
        }
        "git" => {
            let Some(rev) = &entry.rev else {
                return Err(format!("lock entry for {name} missing rev"));
            };
            let Some(git) = &entry.git else {
                return Err(format!("lock entry for {name} missing git url"));
            };
            let base = deps_dir.join(name).join(rev);
            if !base.exists() {
                ensure_checkout(git, rev, &base)?;
            }
            let mut root = base;
            if let Some(subdir) = &entry.subdir {
                root = root.join(subdir);
            }
            Ok(root)
        }
        other => Err(format!("unknown lock source {other} for {name}")),
    }
}

struct NormalizedDependency {
    requested: String,
    kind: NormalizedKind,
}

enum NormalizedKind {
    Path {
        path: PathBuf,
    },
    Git {
        git: String,
        reference: GitReference,
    },
}

struct GitReference {
    requested: GitRequest,
    subdir: Option<String>,
}

enum GitRequest {
    Rev(String),
    Tag(String),
    Branch(String),
    Version(String),
    Head,
}

impl GitReference {
    fn descriptor(&self) -> String {
        match &self.requested {
            GitRequest::Rev(value) => format!("rev:{value}"),
            GitRequest::Tag(value) => format!("tag:{value}"),
            GitRequest::Branch(value) => format!("branch:{value}"),
            GitRequest::Version(value) => format!("version:{value}"),
            GitRequest::Head => "head".to_string(),
        }
    }
}

fn normalize_dependency_spec(
    spec: &DependencySpec,
    base_dir: &Path,
) -> Result<NormalizedDependency, String> {
    let detail = match spec {
        DependencySpec::Simple(value) => {
            if looks_like_git_url(value) {
                DependencyDetail {
                    git: Some(value.clone()),
                    ..DependencyDetail::default()
                }
            } else if looks_like_path(value) {
                DependencyDetail {
                    path: Some(value.clone()),
                    ..DependencyDetail::default()
                }
            } else {
                return Err(format!(
                    "dependency {value} must use {{ git = \"...\" }} or {{ path = \"...\" }}"
                ));
            }
        }
        DependencySpec::Detailed(detail) => detail.clone(),
    };

    if let Some(path) = detail.path {
        if detail.git.is_some()
            || detail.version.is_some()
            || detail.rev.is_some()
            || detail.tag.is_some()
            || detail.branch.is_some()
            || detail.subdir.is_some()
        {
            return Err("path dependencies cannot include git/version fields".to_string());
        }
        let path = resolve_path(base_dir, &path);
        let requested = format!("path:{}", path.display());
        return Ok(NormalizedDependency {
            requested,
            kind: NormalizedKind::Path { path },
        });
    }

    let Some(git) = detail.git else {
        return Err("dependency must specify git or path".to_string());
    };
    if detail.path.is_some() {
        return Err("dependency cannot specify both git and path".to_string());
    }

    let requested_ref = if let Some(rev) = detail.rev {
        GitRequest::Rev(rev)
    } else if let Some(tag) = detail.tag {
        GitRequest::Tag(tag)
    } else if let Some(branch) = detail.branch {
        GitRequest::Branch(branch)
    } else if let Some(version) = detail.version {
        GitRequest::Version(version)
    } else {
        GitRequest::Head
    };
    let reference = GitReference {
        requested: requested_ref,
        subdir: detail.subdir,
    };
    let mut requested = format!("git:{git}|{}", reference.descriptor());
    if let Some(subdir) = &reference.subdir {
        requested.push_str(&format!("|subdir:{subdir}"));
    }
    Ok(NormalizedDependency {
        requested,
        kind: NormalizedKind::Git { git, reference },
    })
}

fn resolve_git_revision(url: &str, reference: &GitReference) -> Result<String, String> {
    match &reference.requested {
        GitRequest::Rev(value) => Ok(value.clone()),
        GitRequest::Tag(tag) => resolve_git_tag(url, tag),
        GitRequest::Branch(branch) => resolve_git_branch(url, branch),
        GitRequest::Version(version) => resolve_git_version(url, version),
        GitRequest::Head => resolve_git_head(url),
    }
}

fn resolve_git_tag(url: &str, tag: &str) -> Result<String, String> {
    let ref_name = format!("refs/tags/{tag}");
    git_ls_remote(url, &ref_name)
}

fn resolve_git_branch(url: &str, branch: &str) -> Result<String, String> {
    let ref_name = format!("refs/heads/{branch}");
    git_ls_remote(url, &ref_name)
}

fn resolve_git_version(url: &str, version: &str) -> Result<String, String> {
    let tag = format!("v{version}");
    if let Ok(rev) = resolve_git_tag(url, &tag) {
        return Ok(rev);
    }
    resolve_git_tag(url, version)
}

fn resolve_git_head(url: &str) -> Result<String, String> {
    git_ls_remote(url, "HEAD")
}

fn git_ls_remote(url: &str, reference: &str) -> Result<String, String> {
    let output = run_git(&["ls-remote", url, reference], None)?;
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        if let Some(hash) = parts.next() {
            return Ok(hash.to_string());
        }
    }
    Err(format!("failed to resolve {reference} for {url}"))
}

fn ensure_checkout(url: &str, rev: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        if !dest.join(".git").exists() {
            return Err(format!(
                "dependency checkout is not a git repo: {}",
                dest.display()
            ));
        }
        let dest_str = dest.to_string_lossy();
        let _ = run_git(&["-C", dest_str.as_ref(), "fetch", "--tags"], None);
        run_git(&["-C", dest_str.as_ref(), "checkout", rev], None)?;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let dest_str = dest.to_string_lossy();
    run_git(&["clone", url, dest_str.as_ref()], None)?;
    run_git(&["-C", dest_str.as_ref(), "checkout", rev], None)?;
    Ok(())
}

fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = ProcessCommand::new("git");
    cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|err| format!("failed to run git {:?}: {err}", args))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {:?} failed: {stderr}", args));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn looks_like_git_url(value: &str) -> bool {
    value.contains("://") || value.starts_with("git@") || value.ends_with(".git")
}

fn looks_like_path(value: &str) -> bool {
    value.starts_with('.') || value.starts_with('/') || value.contains('/')
}

fn resolve_path(base_dir: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn load_lockfile(path: &Path) -> Result<Lockfile, String> {
    if !path.exists() {
        return Ok(Lockfile::default());
    }
    let content = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let lock: Lockfile =
        toml::from_str(&content).map_err(|err| format!("invalid lockfile: {err}"))?;
    if lock.version != 0 && lock.version != 1 {
        return Err(format!("unsupported lockfile version {}", lock.version));
    }
    Ok(lock)
}

fn write_lockfile(path: &Path, lock: &Lockfile) -> Result<(), String> {
    let content =
        toml::to_string_pretty(lock).map_err(|err| format!("lockfile encode failed: {err}"))?;
    fs::write(path, content).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(())
}

fn store_path(root_dir: &Path, path: &Path) -> String {
    if let Ok(stripped) = path.strip_prefix(root_dir) {
        stripped.to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}

fn load_manifest_from_dir(dir: &Path) -> Result<Option<Manifest>, String> {
    let path = dir.join("fuse.toml");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let manifest: Manifest =
        toml::from_str(&content).map_err(|err| format!("invalid manifest: {err}"))?;
    Ok(Some(manifest))
}
