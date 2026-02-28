use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuse_rt::error::{ValidationError, ValidationField};
use fuse_rt::json as rt_json;
use serde::{Deserialize, Serialize};

const USAGE: &str = r#"usage: fuse <command> [options] [file] [-- <program args>]

commands:
  dev       Run package entrypoint with live reload (watch mode)
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
  --backend <ast|native>  Backend override (run only)
  --strict-architecture   Enable strict architectural checks during semantic analysis
  --color <auto|always|never>  Colorized CLI output policy
  --clean                 Remove .fuse/build before building (build only)
  --aot                   Emit deployable AOT binary (build only)
  --release               Use release profile for build output (build only; implies --aot)
"#;

const FUSE_ASSET_MAP_ENV: &str = "FUSE_ASSET_MAP";
const BUILD_TARGET_FINGERPRINT: &str = env!("FUSE_BUILD_TARGET");
const BUILD_RUSTC_FINGERPRINT: &str = env!("FUSE_BUILD_RUSTC_VERSION");
const BUILD_CLI_VERSION_FINGERPRINT: &str = env!("CARGO_PKG_VERSION");
const AOT_SEMANTIC_CONTRACT_VERSION: &str = "aot-v1";

#[derive(Debug, Deserialize)]
struct Manifest {
    package: PackageConfig,
    #[serde(default)]
    build: Option<BuildConfig>,
    #[serde(default)]
    serve: Option<ServeConfig>,
    #[serde(default)]
    assets: Option<AssetsConfig>,
    #[serde(default)]
    vite: Option<ViteConfig>,
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
    openapi_ui: Option<bool>,
    openapi_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AssetsConfig {
    css: Option<String>,
    watch: Option<bool>,
    hash: Option<bool>,
    #[serde(default)]
    hooks: Option<AssetHooksConfig>,
}

#[derive(Debug, Deserialize)]
struct AssetHooksConfig {
    before_build: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ViteConfig {
    dev_url: Option<String>,
    dist_dir: Option<String>,
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
    #[serde(default)]
    manifest_hash: Option<String>,
    #[serde(default)]
    lock_hash: Option<String>,
    #[serde(default)]
    build_target: String,
    #[serde(default)]
    rustc_version: String,
    #[serde(default)]
    cli_version: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct IrFileMeta {
    path: String,
    #[serde(default)]
    hash: String,
}

struct BuildArtifacts {
    native: fusec::native::NativeProgram,
    meta: IrMeta,
}

#[derive(Default)]
struct CommonArgs {
    manifest_path: Option<PathBuf>,
    entry: Option<String>,
    app: Option<String>,
    backend: Option<String>,
    color: Option<ColorChoice>,
    program_args: Vec<String>,
    clean: bool,
    aot: bool,
    release: bool,
    strict_architecture: bool,
}

#[derive(Default)]
struct RawProgramArgs {
    values: HashMap<String, Vec<String>>,
    bools: HashMap<String, bool>,
}

#[derive(Copy, Clone)]
enum Command {
    Dev,
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
    Native,
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum ColorChoice {
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    fn as_env_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

static COLOR_MODE: AtomicU8 = AtomicU8::new(0);

fn apply_color_choice(choice: ColorChoice) {
    let mode = match choice {
        ColorChoice::Always => 2,
        ColorChoice::Never => 0,
        ColorChoice::Auto => {
            if env::var_os("NO_COLOR").is_some() {
                0
            } else if color_auto_is_tty() {
                1
            } else {
                0
            }
        }
    };
    COLOR_MODE.store(mode, Ordering::Relaxed);
    unsafe {
        env::set_var("FUSE_COLOR", choice.as_env_value());
    }
}

fn color_auto_is_tty() -> bool {
    if let Some(force) = env::var_os("FUSE_COLOR_FORCE_TTY") {
        return force == "1";
    }
    std::io::stderr().is_terminal()
}

fn color_enabled() -> bool {
    COLOR_MODE.load(Ordering::Relaxed) != 0
}

fn ansi_paint(text: &str, code: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn style_error(text: &str) -> String {
    ansi_paint(text, "31;1")
}

fn style_warning(text: &str) -> String {
    ansi_paint(text, "33;1")
}

fn style_header(text: &str) -> String {
    ansi_paint(text, "36;1")
}

fn emit_cli_error(message: &str) {
    eprintln!("{}", style_error(&format!("error: {message}")));
}

fn emit_cli_warning(message: &str) {
    eprintln!("{}", style_warning(&format!("warning: {message}")));
}

fn dev_prefix() -> String {
    style_header("[dev]")
}

fn command_tag(command: Command) -> Option<&'static str> {
    match command {
        Command::Run => Some("run"),
        Command::Check => Some("check"),
        Command::Build => Some("build"),
        Command::Test => Some("test"),
        Command::Dev | Command::Fmt | Command::Openapi | Command::Migrate => None,
    }
}

fn command_prefix(command: Command) -> Option<String> {
    command_tag(command).map(|tag| style_header(&format!("[{tag}]")))
}

fn emit_command_step(command: Command, message: &str) {
    if let Some(prefix) = command_prefix(command) {
        eprintln!("{prefix} {message}");
    }
}

fn finalize_command(command: Command, code: i32) -> i32 {
    match code {
        0 => emit_command_step(command, "ok"),
        2 => emit_command_step(command, "validation failed"),
        _ => emit_command_step(command, "failed"),
    }
    code
}

impl RunBackend {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "ast" => Some(Self::Ast),
            "native" => Some(Self::Native),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ast => "ast",
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
    apply_color_choice(ColorChoice::Auto);
    if args.is_empty() {
        eprintln!("{}", style_header(USAGE));
        return 1;
    }
    let (cmd, rest) = args.split_first().unwrap();
    let command = match cmd.as_str() {
        "dev" => Command::Dev,
        "run" => Command::Run,
        "test" => Command::Test,
        "build" => Command::Build,
        "check" => Command::Check,
        "fmt" => Command::Fmt,
        "openapi" => Command::Openapi,
        "migrate" => Command::Migrate,
        _ => {
            emit_cli_error(&format!("unknown command: {cmd}"));
            eprintln!("{}", style_header(USAGE));
            return 1;
        }
    };
    if let Some(choice) = discover_color_choice(rest) {
        apply_color_choice(choice);
    }
    let allow_program_args = matches!(command, Command::Run);
    let allow_clean = matches!(command, Command::Build);
    let allow_build_mode = matches!(command, Command::Build);
    let common = match parse_common_args(rest, allow_program_args, allow_clean, allow_build_mode) {
        Ok(args) => args,
        Err(err) => {
            emit_cli_error(&err);
            eprintln!("{}", style_header(USAGE));
            return 1;
        }
    };
    apply_color_choice(common.color.unwrap_or(ColorChoice::Auto));

    let (manifest, manifest_dir) = match load_manifest(common.manifest_path.as_deref()) {
        Ok(value) => value,
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };
    apply_dotenv(manifest_dir.as_deref());
    apply_default_config_path(manifest_dir.as_deref());

    let entry = resolve_entry(&common, manifest.as_ref(), manifest_dir.as_deref());
    let entry = match entry {
        Ok(entry) => entry,
        Err(err) => {
            emit_cli_error(&err);
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
                emit_cli_error(&format!("unknown backend: {name}"));
                return 1;
            }
        }
    } else {
        None
    };

    let backend_flag = backend.map(|backend| backend.as_str().to_string());

    let deps = match resolve_dependencies(manifest.as_ref(), manifest_dir.as_deref()) {
        Ok(deps) => deps,
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };

    if matches!(command, Command::Run) {
        let dev_mode = env::var("FUSE_DEV_MODE")
            .ok()
            .as_deref()
            .map(|value| value == "1")
            .unwrap_or(false);
        if let Err(err) = configure_openapi_ui_env(
            &entry,
            manifest.as_ref(),
            manifest_dir.as_deref(),
            &deps,
            dev_mode,
        ) {
            emit_cli_error(&err);
            return 1;
        }
    }

    emit_command_step(command, "start");

    if matches!(command, Command::Run) {
        match backend {
            Some(RunBackend::Ast) => {}
            _ => {
                if let Some(native) = try_load_native(manifest_dir.as_deref()) {
                    apply_serve_env(manifest.as_ref(), manifest_dir.as_deref());
                    return finalize_command(
                        command,
                        run_native_program(
                            native,
                            app.as_deref(),
                            &entry,
                            &deps,
                            &common.program_args,
                        ),
                    );
                }
            }
        }
    }

    let code = match command {
        Command::Dev => run_dev(
            &entry,
            manifest.as_ref(),
            manifest_dir.as_deref(),
            &deps,
            app.as_deref(),
            backend,
        ),
        Command::Run => {
            apply_serve_env(manifest.as_ref(), manifest_dir.as_deref());
            let mut args = Vec::new();
            args.push("--run".to_string());
            if common.strict_architecture {
                args.push("--strict-architecture".to_string());
            }
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
            if common.strict_architecture {
                args.push("--strict-architecture".to_string());
            }
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
            common.aot,
            common.release,
            common.strict_architecture,
        ),
        Command::Check => {
            if common.entry.is_none() && manifest.is_some() {
                run_project_check(&entry, &deps, common.strict_architecture)
            } else {
                let mut args = Vec::new();
                args.push("--check".to_string());
                if common.strict_architecture {
                    args.push("--strict-architecture".to_string());
                }
                args.push(entry.to_string_lossy().to_string());
                fusec::cli::run_with_deps(args, Some(&deps))
            }
        }
        Command::Fmt => {
            if common.entry.is_none() && manifest.is_some() {
                run_project_fmt(&entry, &deps)
            } else {
                let mut args = Vec::new();
                args.push("--fmt".to_string());
                args.push(entry.to_string_lossy().to_string());
                fusec::cli::run_with_deps(args, Some(&deps))
            }
        }
        Command::Openapi => {
            let mut args = Vec::new();
            args.push("--openapi".to_string());
            if common.strict_architecture {
                args.push("--strict-architecture".to_string());
            }
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
        Command::Migrate => {
            let mut args = Vec::new();
            args.push("--migrate".to_string());
            if common.strict_architecture {
                args.push("--strict-architecture".to_string());
            }
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
    };

    finalize_command(command, code)
}

fn discover_color_choice(args: &[String]) -> Option<ColorChoice> {
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            break;
        }
        if arg == "--color" {
            idx += 1;
            let value = args.get(idx)?;
            return ColorChoice::parse(value);
        }
        if let Some(value) = arg.strip_prefix("--color=") {
            return ColorChoice::parse(value);
        }
        idx += 1;
    }
    None
}

fn configure_openapi_ui_env(
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
        emit_diags(&diags);
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

fn run_dev(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    app: Option<&str>,
    backend: Option<RunBackend>,
) -> i32 {
    let reload = match ReloadHub::start() {
        Ok(reload) => reload,
        Err(err) => {
            emit_cli_error(&format!("dev error: {err}"));
            return 1;
        }
    };
    let ws_url = reload.ws_url();
    eprintln!("{} live reload websocket: {ws_url}", dev_prefix());

    if let Err(err) = run_asset_pipeline(manifest, manifest_dir) {
        eprintln!("{} {}", dev_prefix(), style_error(&err));
    }

    let mut snapshot = build_dev_snapshot(entry, manifest, manifest_dir, deps);
    let mut child = match spawn_dev_child(entry, manifest_dir, app, backend, &ws_url) {
        Ok(child) => Some(child),
        Err(err) => {
            emit_cli_error(&err);
            None
        }
    };
    let mut child_exit_reported = false;

    loop {
        thread::sleep(Duration::from_millis(300));

        if let Some(proc) = child.as_mut() {
            match proc.try_wait() {
                Ok(Some(status)) => {
                    if !child_exit_reported {
                        eprintln!(
                            "{} app exited ({status}); waiting for changes...",
                            dev_prefix()
                        );
                        child_exit_reported = true;
                    }
                    child = None;
                }
                Ok(None) => {}
                Err(err) => {
                    if !child_exit_reported {
                        eprintln!("{} failed to poll app process: {err}", dev_prefix());
                        child_exit_reported = true;
                    }
                    child = None;
                }
            }
        }

        let next_snapshot = build_dev_snapshot(entry, manifest, manifest_dir, deps);
        if next_snapshot == snapshot {
            continue;
        }

        snapshot = next_snapshot;
        eprintln!("{} change detected, restarting...", dev_prefix());
        if let Err(err) = run_asset_pipeline(manifest, manifest_dir) {
            eprintln!("{} {}", dev_prefix(), style_error(&err));
        }
        child_exit_reported = false;
        if let Some(mut proc) = child.take() {
            let _ = proc.kill();
            let _ = proc.wait();
        }
        match spawn_dev_child(entry, manifest_dir, app, backend, &ws_url) {
            Ok(proc) => {
                child = Some(proc);
                reload.broadcast_reload();
            }
            Err(err) => {
                emit_cli_error(&err);
            }
        }
    }
}

fn spawn_dev_child(
    entry: &Path,
    manifest_dir: Option<&Path>,
    app: Option<&str>,
    backend: Option<RunBackend>,
    ws_url: &str,
) -> Result<Child, String> {
    let exe = env::current_exe().map_err(|err| format!("dev error: current exe: {err}"))?;
    let mut cmd = ProcessCommand::new(exe);
    cmd.arg("run");
    if let Some(dir) = manifest_dir {
        cmd.arg("--manifest-path");
        cmd.arg(dir);
    }
    cmd.arg("--file");
    cmd.arg(entry);
    if let Some(name) = app {
        cmd.arg("--app");
        cmd.arg(name);
    }
    if let Some(backend) = backend {
        cmd.arg("--backend");
        cmd.arg(backend.as_str());
    }
    cmd.env("FUSE_DEV_MODE", "1");
    cmd.env("FUSE_DEV_RELOAD_WS_URL", ws_url);
    cmd.spawn()
        .map_err(|err| format!("dev error: failed to start app: {err}"))
}

#[derive(Clone, Default, Eq, PartialEq)]
struct DevSnapshot {
    files: BTreeMap<PathBuf, Option<FileStamp>>,
}

fn build_dev_snapshot(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
) -> DevSnapshot {
    let files = collect_dev_watch_files(entry, manifest, manifest_dir, deps);
    let mut stamps = BTreeMap::new();
    for file in files {
        stamps.insert(file.clone(), file_stamp(&file).ok());
    }
    DevSnapshot { files: stamps }
}

fn collect_dev_watch_files(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
) -> BTreeSet<PathBuf> {
    let mut out = collect_module_files_for_dev(entry, deps);
    if out.is_empty() {
        out.insert(entry.to_path_buf());
        if let Some(base) = manifest_dir.or_else(|| entry.parent()) {
            collect_files_by_extension(base, &["fuse"], &mut out);
        }
    }
    if let Some(base) = manifest_dir.or_else(|| entry.parent()) {
        if let Some(assets) = manifest.and_then(|m| m.assets.as_ref()) {
            if assets.watch != Some(false) {
                if let Some(css) = assets.css.as_ref() {
                    let path = resolve_manifest_relative_path(base, css);
                    if path.is_dir() {
                        collect_files_by_extension(&path, &["css"], &mut out);
                    } else if path.is_file() {
                        out.insert(path.clone());
                        if let Some(parent) = path.parent() {
                            collect_files_by_extension(parent, &["css"], &mut out);
                        }
                    }
                }
            }
        }
    }
    out
}

fn collect_module_files_for_dev(
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let src = match fs::read_to_string(entry) {
        Ok(src) => src,
        Err(_) => return out,
    };
    let (registry, _diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    for unit in registry.modules.values() {
        if unit.path.exists() {
            out.insert(unit.path.clone());
        }
    }
    out
}

fn collect_files_by_extension(root: &Path, exts: &[&str], out: &mut BTreeSet<PathBuf>) {
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

fn resolve_manifest_relative_path(base: &Path, path: &str) -> PathBuf {
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

fn run_before_build_hook(
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

fn run_asset_pipeline(
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
    let digest = sha1_digest(&bytes);
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

struct ReloadHub {
    addr: String,
    clients: Arc<Mutex<Vec<TcpStream>>>,
}

impl ReloadHub {
    fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|err| format!("failed to bind reload websocket: {err}"))?;
        let addr = listener
            .local_addr()
            .map_err(|err| format!("failed to read reload websocket address: {err}"))?;
        let clients = Arc::new(Mutex::new(Vec::new()));
        let thread_clients = Arc::clone(&clients);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                handle_reload_client(stream, &thread_clients);
            }
        });
        Ok(Self {
            addr: addr.to_string(),
            clients,
        })
    }

    fn ws_url(&self) -> String {
        format!("ws://{}/__reload", self.addr)
    }

    fn broadcast_reload(&self) {
        let frame = websocket_text_frame("reload");
        let mut clients = match self.clients.lock() {
            Ok(clients) => clients,
            Err(_) => return,
        };
        let mut idx = 0usize;
        while idx < clients.len() {
            if clients[idx].write_all(&frame).is_err() {
                clients.remove(idx);
            } else {
                idx += 1;
            }
        }
    }
}

fn handle_reload_client(mut stream: TcpStream, clients: &Arc<Mutex<Vec<TcpStream>>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let header = match read_http_header(&mut stream) {
        Ok(header) => header,
        Err(_) => return,
    };
    let header = String::from_utf8_lossy(&header);
    let mut lines = header.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if method != "GET" || !path.starts_with("/__reload") {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let mut upgrade = false;
    let mut connection_upgrade = false;
    let mut ws_key = None::<String>;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "upgrade" if value.eq_ignore_ascii_case("websocket") => {
                    upgrade = true;
                }
                "connection" if value.to_ascii_lowercase().contains("upgrade") => {
                    connection_upgrade = true;
                }
                "sec-websocket-key" => {
                    ws_key = Some(value.to_string());
                }
                _ => {}
            }
        }
    }
    let Some(ws_key) = ws_key else {
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    if !upgrade || !connection_upgrade {
        let _ = stream.write_all(b"HTTP/1.1 426 Upgrade Required\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let accept = websocket_accept_value(&ws_key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(response.as_bytes()).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_nonblocking(true);
    if let Ok(mut guard) = clients.lock() {
        guard.push(stream);
    }
}

fn read_http_header(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let read = stream.read(&mut temp)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() >= 16 * 1024 {
            break;
        }
    }
    Ok(buffer)
}

fn websocket_accept_value(key: &str) -> String {
    let mut combined = String::new();
    combined.push_str(key.trim());
    combined.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let digest = sha1_digest(combined.as_bytes());
    fuse_rt::bytes::encode_base64(&digest)
}

fn websocket_text_frame(payload: &str) -> Vec<u8> {
    let bytes = payload.as_bytes();
    let mut frame = Vec::with_capacity(bytes.len() + 10);
    frame.push(0x81);
    match bytes.len() {
        len if len <= 125 => frame.push(len as u8),
        len if len <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            frame.push(127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(bytes);
    frame
}

fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xEFCD_AB89;
    let mut h2: u32 = 0x98BA_DCFE;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xC3D2_E1F0;

    let mut data = input.to_vec();
    data.push(0x80);
    while (data.len() % 64) != 56 {
        data.push(0);
    }
    let bit_len = (input.len() as u64) * 8;
    data.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in data.chunks(64) {
        let mut words = [0u32; 80];
        for (i, word) in words.iter_mut().enumerate().take(16) {
            let base = i * 4;
            *word = u32::from_be_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
        }
        for i in 16..80 {
            words[i] = (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for (i, word) in words.iter().enumerate() {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A82_7999)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9_EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC)
            } else {
                (b ^ c ^ d, 0xCA62_C1D6)
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

fn apply_serve_env(manifest: Option<&Manifest>, manifest_dir: Option<&Path>) {
    apply_asset_manifest_env(manifest_dir);
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

fn apply_asset_manifest_env(manifest_dir: Option<&Path>) {
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

fn apply_dotenv(manifest_dir: Option<&Path>) {
    let mut path = PathBuf::from(".env");
    if let Some(dir) = manifest_dir {
        path = dir.join(".env");
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            emit_cli_warning(&format!("failed to read {}: {err}", path.display()));
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
    allow_build_mode: bool,
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
        if arg == "--color" {
            idx += 1;
            let Some(choice) = args.get(idx) else {
                return Err("--color expects auto, always, or never".to_string());
            };
            let Some(parsed) = ColorChoice::parse(choice) else {
                return Err(format!(
                    "invalid --color value: {choice} (expected auto|always|never)"
                ));
            };
            out.color = Some(parsed);
            idx += 1;
            continue;
        }
        if let Some(choice) = arg.strip_prefix("--color=") {
            let Some(parsed) = ColorChoice::parse(choice) else {
                return Err(format!(
                    "invalid --color value: {choice} (expected auto|always|never)"
                ));
            };
            out.color = Some(parsed);
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
        if arg == "--strict-architecture" {
            out.strict_architecture = true;
            idx += 1;
            continue;
        }
        if arg == "--aot" {
            if !allow_build_mode {
                return Err("--aot is only supported for fuse build".to_string());
            }
            out.aot = true;
            idx += 1;
            continue;
        }
        if arg == "--release" {
            if !allow_build_mode {
                return Err("--release is only supported for fuse build".to_string());
            }
            out.release = true;
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
    aot: bool,
    release: bool,
    strict_architecture: bool,
) -> i32 {
    if clean {
        if let Err(err) = clean_build_dir(manifest_dir) {
            emit_cli_error(&err);
            return 1;
        }
        return 0;
    }
    if let Err(err) = run_before_build_hook(manifest, manifest_dir) {
        emit_cli_error(&err);
        return 1;
    }
    if let Err(err) = run_asset_pipeline(manifest, manifest_dir) {
        emit_cli_error(&err);
        return 1;
    }
    let artifacts = match compile_artifacts(entry, manifest_dir, deps, strict_architecture) {
        Ok(artifacts) => artifacts,
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };
    if let Err(err) = write_compiled_artifacts(manifest_dir, &artifacts) {
        emit_cli_error(&err);
        return 1;
    }
    let configured_native_bin =
        manifest.and_then(|m| m.build.as_ref().and_then(|b| b.native_bin.clone()));
    let aot_enabled = aot || release;
    let aot_out = configured_native_bin.or_else(|| {
        if aot_enabled {
            Some(default_aot_output_path())
        } else {
            None
        }
    });
    if let Some(native_bin) = aot_out {
        if let Err(err) =
            write_native_binary(manifest_dir, &artifacts.native, app, &native_bin, release)
        {
            emit_cli_error(&err);
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
                    emit_cli_error(&format!("cwd error: {err}"));
                    return 1;
                }
            }
        }
    };
    if let Err(err) = write_openapi(entry, &out_path, deps) {
        emit_cli_error(&err);
        return 1;
    }
    0
}

fn run_project_check(
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
    strict_architecture: bool,
) -> i32 {
    let files = match collect_project_files(entry, deps) {
        Ok(files) => files,
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };
    let mut had_errors = false;
    for file in files {
        let src = match fs::read_to_string(&file) {
            Ok(src) => src,
            Err(err) => {
                emit_cli_error(&format!("failed to read {}: {err}", file.display()));
                return 1;
            }
        };
        let (registry, diags) = fusec::load_program_with_modules_and_deps(&file, &src, deps);
        if !diags.is_empty() {
            emit_diags_with_fallback(&diags, Some((&file, &src)));
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
            emit_diags_with_fallback(&diags, Some((&file, &src)));
            had_errors = true;
        }
    }
    if had_errors { 1 } else { 0 }
}

fn run_project_fmt(entry: &Path, deps: &HashMap<String, PathBuf>) -> i32 {
    let files = match collect_project_files(entry, deps) {
        Ok(files) => files,
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };
    for file in files {
        let src = match fs::read_to_string(&file) {
            Ok(src) => src,
            Err(err) => {
                emit_cli_error(&format!("failed to read {}: {err}", file.display()));
                return 1;
            }
        };
        let formatted = fusec::format::format_source(&src);
        if formatted != src {
            if let Err(err) = fs::write(&file, formatted) {
                emit_cli_error(&format!("failed to write {}: {err}", file.display()));
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
        emit_diags(&diags);
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
        emit_diags(&diags);
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

fn default_aot_output_path() -> String {
    if cfg!(windows) {
        ".fuse/build/program.aot.exe".to_string()
    } else {
        ".fuse/build/program.aot".to_string()
    }
}

fn write_native_binary(
    manifest_dir: Option<&Path>,
    program: &fusec::native::NativeProgram,
    app: Option<&str>,
    out_path: &str,
    release: bool,
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
    let target_literal = format!("{BUILD_TARGET_FINGERPRINT:?}");
    let rustc_literal = format!("{BUILD_RUSTC_FINGERPRINT:?}");
    let cli_literal = format!("{BUILD_CLI_VERSION_FINGERPRINT:?}");
    let contract_literal = format!("{AOT_SEMANTIC_CONTRACT_VERSION:?}");
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
    load_configs(&mut heap)?;
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
    let mut build_cmd = ProcessCommand::new("cargo");
    apply_toolchain_env(
        &mut build_cmd,
        &target_dir,
        &rustc_tmpdir,
        &cargo_incremental,
    );
    build_cmd
        .arg("build")
        .arg("-p")
        // `fusec` has a direct `bincode` dependency so both rlibs are
        // available in `<target>/<profile>/deps` for the runner link step.
        .arg("fusec");
    if release {
        build_cmd.arg("--release");
    }
    let build_output = build_cmd
        .output()
        .map_err(|err| format!("failed to run cargo build for link dependencies: {err}"))?;
    if !build_output.status.success() {
        return Err(format!(
            "native link: failed to build link dependencies\n{}",
            summarize_command_failure(&build_output)
        ));
    }
    let profile = if release { "release" } else { "debug" };
    let deps_dir = target_dir.join(profile).join("deps");
    let fusec_rlib = find_latest_rlib(&deps_dir, "libfusec")?;
    let bincode_rlib = find_latest_rlib(&deps_dir, "libbincode")?;
    let native_link_searches = collect_rustc_link_search_paths(&target_dir, profile)?;
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
        .arg(format!("dependency={}", deps_dir.display()))
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

fn compile_artifacts(
    entry: &Path,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    strict_architecture: bool,
) -> Result<BuildArtifacts, String> {
    let src = fs::read_to_string(entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        emit_diags(&diags);
        return Err("build failed".to_string());
    }
    let (_analysis, sema_diags) = fusec::sema::analyze_registry_with_options(
        &registry,
        fusec::sema::AnalyzeOptions {
            strict_architecture,
        },
    );
    if !sema_diags.is_empty() {
        emit_diags(&sema_diags);
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
    let meta = build_ir_meta(&registry, manifest_dir)?;
    Ok(BuildArtifacts { native, meta })
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

fn try_load_native(manifest_dir: Option<&Path>) -> Option<fusec::native::NativeProgram> {
    let build_dir = build_dir(manifest_dir).ok()?;
    let path = build_dir.join("program.native");
    let meta_path = build_dir.join("program.meta");
    let meta = load_ir_meta(&meta_path)?;
    if !ir_meta_is_valid(&meta, manifest_dir) {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let native: fusec::native::NativeProgram = bincode::deserialize(&bytes).ok()?;
    if native.version != fusec::native::NativeProgram::VERSION {
        return None;
    }
    Some(native)
}

fn run_native_program(
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
                emit_cli_error(&format!("run error: {err}"));
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
            emit_cli_error(&format!("failed to read {}: {err}", entry.display()));
            return Err(1);
        }
    };
    let (registry, diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !diags.is_empty() {
        emit_diags_with_fallback(&diags, Some((entry, &src)));
        return Err(1);
    }
    let root = match registry.root() {
        Some(root) => root,
        None => {
            emit_cli_error("no root module loaded");
            return Err(1);
        }
    };
    let main_decl = root.program.items.iter().find_map(|item| match item {
        fusec::ast::Item::Fn(decl) if decl.name.name == "main" => Some(decl),
        _ => None,
    });
    let Some(main_decl) = main_decl else {
        emit_cli_error("no root fn main found for CLI binding");
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
    for (name, _) in raw.values.iter() {
        if !param_names.contains(name) {
            errors.push(ValidationField {
                path: name.clone(),
                code: "unknown_flag".to_string(),
                message: "unknown flag".to_string(),
            });
        }
    }
    for (name, _) in raw.bools.iter() {
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
        emit_validation_error("$", "runtime_error", message);
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

fn build_ir_meta(
    registry: &fusec::ModuleRegistry,
    manifest_dir: Option<&Path>,
) -> Result<IrMeta, String> {
    let mut files = Vec::new();
    for unit in registry.modules.values() {
        files.push(IrFileMeta {
            path: unit.path.to_string_lossy().to_string(),
            hash: file_hash_hex(&unit.path)?,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest_hash = manifest_dir
        .map(|dir| dir.join("fuse.toml"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()?;
    let lock_hash = manifest_dir
        .map(|dir| dir.join("fuse.lock"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()?;
    Ok(IrMeta {
        version: 3,
        native_cache_version: fusec::native::CACHE_VERSION,
        files,
        manifest_hash,
        lock_hash,
        build_target: BUILD_TARGET_FINGERPRINT.to_string(),
        rustc_version: BUILD_RUSTC_FINGERPRINT.to_string(),
        cli_version: BUILD_CLI_VERSION_FINGERPRINT.to_string(),
    })
}

fn load_ir_meta(path: &Path) -> Option<IrMeta> {
    let bytes = fs::read(path).ok()?;
    bincode::deserialize(&bytes).ok()
}

fn ir_meta_is_valid(meta: &IrMeta, manifest_dir: Option<&Path>) -> bool {
    if meta.version != 3 || meta.files.is_empty() {
        return false;
    }
    if meta.native_cache_version != fusec::native::CACHE_VERSION {
        return false;
    }
    for file in &meta.files {
        let hash = match file_hash_hex(Path::new(&file.path)) {
            Ok(hash) => hash,
            Err(_) => return false,
        };
        if hash != file.hash {
            return false;
        }
    }
    let current_manifest_hash = manifest_dir
        .map(|dir| dir.join("fuse.toml"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()
        .ok()
        .flatten();
    if meta.manifest_hash != current_manifest_hash {
        return false;
    }
    let current_lock_hash = manifest_dir
        .map(|dir| dir.join("fuse.lock"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()
        .ok()
        .flatten();
    if meta.lock_hash != current_lock_hash {
        return false;
    }
    if meta.build_target != BUILD_TARGET_FINGERPRINT {
        return false;
    }
    if meta.rustc_version != BUILD_RUSTC_FINGERPRINT {
        return false;
    }
    if meta.cli_version != BUILD_CLI_VERSION_FINGERPRINT {
        return false;
    }
    true
}

#[derive(Clone, Copy, Eq, PartialEq)]
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

fn file_hash_hex(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    Ok(hash_hex(&sha1_digest(&bytes)))
}

fn optional_file_hash_hex(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(file_hash_hex(path)?))
}

fn hash_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn emit_diags(diags: &[fusec::diag::Diag]) {
    emit_diags_with_fallback(diags, None);
}

fn emit_diags_with_fallback(diags: &[fusec::diag::Diag], fallback: Option<(&Path, &str)>) {
    for diag in diags {
        emit_diag(diag, fallback);
    }
}

fn styled_diag_level(level: &fusec::diag::Level) -> String {
    match level {
        fusec::diag::Level::Error => style_error("error"),
        fusec::diag::Level::Warning => style_warning("warning"),
    }
}

fn emit_diag(diag: &fusec::diag::Diag, fallback: Option<(&Path, &str)>) {
    let level = styled_diag_level(&diag.level);
    if let Some(path) = &diag.path {
        if let Ok(src) = fs::read_to_string(path) {
            let (line, col, line_text) = line_info(&src, diag.span.start);
            eprintln!(
                "{level}: {} ({}:{}:{})",
                diag.message,
                path.display(),
                line,
                col
            );
            eprintln!("  {line_text}");
            eprintln!(
                "  {}{}",
                " ".repeat(col.saturating_sub(1)),
                style_error("^")
            );
            return;
        }
        eprintln!("{level}: {} ({})", diag.message, path.display());
        return;
    }
    if let Some((path, src)) = fallback {
        let (line, col, line_text) = line_info(src, diag.span.start);
        eprintln!(
            "{level}: {} ({}:{}:{})",
            diag.message,
            path.display(),
            line,
            col
        );
        eprintln!("  {line_text}");
        eprintln!(
            "  {}{}",
            " ".repeat(col.saturating_sub(1)),
            style_error("^")
        );
        return;
    }
    eprintln!(
        "{level}: {} ({}..{})",
        diag.message, diag.span.start, diag.span.end
    );
}

fn line_info(src: &str, offset: usize) -> (usize, usize, &str) {
    let offset = offset.min(src.len());
    let mut line = 1usize;
    let mut line_start = 0usize;
    for (idx, byte) in src.bytes().enumerate() {
        if idx >= offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = idx + 1;
        }
    }
    let line_end = src[line_start..]
        .find('\n')
        .map(|rel| line_start + rel)
        .unwrap_or(src.len());
    let col = offset.saturating_sub(line_start) + 1;
    let line_text = &src[line_start..line_end];
    (line, col, line_text)
}

fn dep_error(code: &str, message: impl Into<String>) -> String {
    format!("[{code}] {}", message.into())
}

fn dep_error_with_hint(code: &str, message: impl Into<String>, hint: &str) -> String {
    format!("[{code}] {}. Hint: {hint}", message.into())
}

fn lockfile_remediation_hint() -> &'static str {
    "delete fuse.lock and rerun 'fuse build' (or run 'fuse build --clean')"
}

fn lock_error(code: &str, message: impl Into<String>) -> String {
    dep_error_with_hint(code, message, lockfile_remediation_hint())
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
        return Err(dep_error(
            "FUSE_DEP_MANIFEST_DIR_REQUIRED",
            "dependencies require a manifest directory",
        ));
    };
    let lock_path = root_dir.join("fuse.lock");
    let mut lock = load_lockfile(&lock_path)?;
    let deps_dir = root_dir.join(".fuse").join("deps");
    if !deps_dir.exists() {
        fs::create_dir_all(&deps_dir).map_err(|err| {
            dep_error(
                "FUSE_DEP_CACHE_DIR_CREATE_FAILED",
                format!("failed to create {}: {err}", deps_dir.display()),
            )
        })?;
    }

    let mut resolved = HashMap::new();
    let mut requests: HashMap<String, (String, PathBuf)> = HashMap::new();
    let mut queue: VecDeque<(String, DependencySpec, PathBuf)> = VecDeque::new();
    for (name, spec) in &manifest.dependencies {
        queue.push_back((name.clone(), spec.clone(), root_dir.to_path_buf()));
    }

    while let Some((name, spec, base_dir)) = queue.pop_front() {
        let requested = dependency_request_key(&name, &spec, &base_dir)?;
        if let Some((prev, prev_base_dir)) = requests.get(&name) {
            if prev != &requested {
                let prev_from = prev_base_dir.join("fuse.toml");
                let next_from = base_dir.join("fuse.toml");
                return Err(dep_error(
                    "FUSE_DEP_CONFLICTING_SPECS",
                    format!(
                        "dependency {name} requested with conflicting specs:\n  - {prev} (from {})\n  - {requested} (from {})",
                        prev_from.display(),
                        next_from.display()
                    ),
                ));
            }
        } else {
            requests.insert(name.clone(), (requested.clone(), base_dir.clone()));
        }
        if resolved.contains_key(&name) {
            continue;
        }
        let root = resolve_dependency(&name, &spec, &base_dir, root_dir, &deps_dir, &mut lock)?;
        resolved.insert(name.clone(), root.clone());

        if let Some(dep_manifest) = load_manifest_from_dir(&root).map_err(|err| {
            dep_error(
                "FUSE_DEP_MANIFEST_LOAD_FAILED",
                format!(
                    "failed to load dependency manifest from {}: {err}",
                    root.display()
                ),
            )
        })? {
            for (dep_name, dep_spec) in dep_manifest.dependencies {
                queue.push_back((dep_name, dep_spec, root.clone()));
            }
        }
    }

    lock.version = 1;
    write_lockfile(&lock_path, &lock)?;
    Ok(resolved)
}

fn dependency_request_key(
    name: &str,
    spec: &DependencySpec,
    base_dir: &Path,
) -> Result<String, String> {
    let normalized = normalize_dependency_spec(name, spec, base_dir)?;
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
    let normalized = normalize_dependency_spec(name, spec, base_dir)?;
    if let Some(entry) = lock.dependencies.get(name) {
        if entry.requested.as_deref() == Some(normalized.requested.as_str()) {
            return root_from_lock(name, entry, root_dir, deps_dir);
        }
    }

    let (root, entry) = match normalized.kind {
        NormalizedKind::Path { path } => {
            if !path.exists() {
                return Err(dep_error_with_hint(
                    "FUSE_DEP_PATH_NOT_FOUND",
                    format!("dependency {name} path does not exist: {}", path.display()),
                    "fix the dependency path in fuse.toml",
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
                return Err(dep_error_with_hint(
                    "FUSE_DEP_SUBDIR_NOT_FOUND",
                    format!(
                        "dependency {name} subdir does not exist: {}",
                        root.display()
                    ),
                    "fix the dependency subdir in fuse.toml",
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
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_MISSING_PATH",
                    format!("lock entry for {name} missing path"),
                ));
            };
            let path = PathBuf::from(path);
            let resolved = if path.is_absolute() {
                path
            } else {
                root_dir.join(path)
            };
            if !resolved.exists() {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_PATH_NOT_FOUND",
                    format!(
                        "lock entry for {name} points to missing path: {}",
                        resolved.display()
                    ),
                ));
            }
            Ok(resolved)
        }
        "git" => {
            let Some(rev) = &entry.rev else {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_MISSING_REV",
                    format!("lock entry for {name} missing rev"),
                ));
            };
            let Some(git) = &entry.git else {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_MISSING_GIT",
                    format!("lock entry for {name} missing git url"),
                ));
            };
            let base = deps_dir.join(name).join(rev);
            if !base.exists() {
                ensure_checkout(git, rev, &base)?;
            }
            let mut root = base;
            if let Some(subdir) = &entry.subdir {
                root = root.join(subdir);
            }
            if !root.exists() {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_SUBDIR_NOT_FOUND",
                    format!(
                        "lock entry for {name} points to missing git path: {}",
                        root.display()
                    ),
                ));
            }
            Ok(root)
        }
        other => Err(lock_error(
            "FUSE_LOCK_ENTRY_UNKNOWN_SOURCE",
            format!("unknown lock source {other} for {name}"),
        )),
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
    name: &str,
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
                return Err(dep_error(
                    "FUSE_DEP_INVALID_SOURCE",
                    format!(
                        "dependency {name} has invalid source {value:?}; use a relative/absolute path or {{ git = \"...\" }}"
                    ),
                ));
            }
        }
        DependencySpec::Detailed(detail) => detail.clone(),
    };

    if let Some(path) = detail.path {
        if path.trim().is_empty() {
            return Err(dep_error(
                "FUSE_DEP_PATH_EMPTY",
                format!("dependency {name} path cannot be empty"),
            ));
        }
        if detail.git.is_some()
            || detail.version.is_some()
            || detail.rev.is_some()
            || detail.tag.is_some()
            || detail.branch.is_some()
            || detail.subdir.is_some()
        {
            return Err(dep_error(
                "FUSE_DEP_PATH_FIELDS_INVALID",
                format!(
                    "dependency {name} path dependencies cannot include git/rev/tag/branch/version/subdir fields"
                ),
            ));
        }
        let path = resolve_path(base_dir, &normalize_dependency_path_input(&path));
        let normalized_path = canonicalize_dependency_path_for_requested(&path);
        let requested = format!("path:{}", normalized_path.display());
        return Ok(NormalizedDependency {
            requested,
            kind: NormalizedKind::Path {
                path: normalized_path,
            },
        });
    }

    let has_ref = detail.rev.is_some()
        || detail.tag.is_some()
        || detail.branch.is_some()
        || detail.version.is_some();
    let Some(git) = detail.git else {
        if has_ref || detail.subdir.is_some() {
            return Err(dep_error(
                "FUSE_DEP_GIT_REQUIRED_FOR_REFS",
                format!(
                    "dependency {name} must specify git when using rev/tag/branch/version/subdir"
                ),
            ));
        }
        return Err(dep_error(
            "FUSE_DEP_SOURCE_REQUIRED",
            format!("dependency {name} must specify either path or git"),
        ));
    };
    if git.trim().is_empty() {
        return Err(dep_error(
            "FUSE_DEP_GIT_EMPTY",
            format!("dependency {name} git url cannot be empty"),
        ));
    }
    if detail.path.is_some() {
        return Err(dep_error(
            "FUSE_DEP_GIT_PATH_CONFLICT",
            format!("dependency {name} cannot specify both git and path"),
        ));
    }
    if let Some(subdir) = detail.subdir.as_ref() {
        if subdir.trim().is_empty() {
            return Err(dep_error(
                "FUSE_DEP_SUBDIR_EMPTY",
                format!("dependency {name} subdir cannot be empty"),
            ));
        }
    }

    let mut selected_refs = 0usize;
    if detail.rev.is_some() {
        selected_refs += 1;
    }
    if detail.tag.is_some() {
        selected_refs += 1;
    }
    if detail.branch.is_some() {
        selected_refs += 1;
    }
    if detail.version.is_some() {
        selected_refs += 1;
    }
    if selected_refs > 1 {
        return Err(dep_error(
            "FUSE_DEP_GIT_REF_CONFLICT",
            format!("dependency {name} must specify at most one of rev, tag, branch, version"),
        ));
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
    Err(dep_error(
        "FUSE_DEP_GIT_REF_RESOLVE_FAILED",
        format!("failed to resolve {reference} for {url}"),
    ))
}

fn ensure_checkout(url: &str, rev: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        if !dest.join(".git").exists() {
            return Err(dep_error_with_hint(
                "FUSE_DEP_CHECKOUT_NOT_GIT",
                format!("dependency checkout is not a git repo: {}", dest.display()),
                "remove .fuse/deps for this package and rerun the command",
            ));
        }
        let dest_str = dest.to_string_lossy();
        let _ = run_git(&["-C", dest_str.as_ref(), "fetch", "--tags"], None);
        run_git(&["-C", dest_str.as_ref(), "checkout", rev], None)?;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            dep_error(
                "FUSE_DEP_CHECKOUT_DIR_CREATE_FAILED",
                format!("failed to create {}: {err}", parent.display()),
            )
        })?;
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
    let output = cmd.output().map_err(|err| {
        dep_error(
            "FUSE_DEP_GIT_COMMAND_START_FAILED",
            format!("failed to run git {:?}: {err}", args),
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(dep_error(
            "FUSE_DEP_GIT_COMMAND_FAILED",
            format!("git {:?} failed: {stderr}", args),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn looks_like_git_url(value: &str) -> bool {
    value.contains("://") || value.starts_with("git@") || value.ends_with(".git")
}

fn looks_like_path(value: &str) -> bool {
    value.starts_with('.') || value.starts_with('/') || value.contains('/') || value.contains('\\')
}

fn resolve_path(base_dir: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn normalize_dependency_path_input(raw: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        raw.to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        raw.replace('\\', "/")
    }
}

fn canonicalize_dependency_path_for_requested(path: &Path) -> PathBuf {
    if path.exists() {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn load_lockfile(path: &Path) -> Result<Lockfile, String> {
    if !path.exists() {
        return Ok(Lockfile::default());
    }
    let content = fs::read_to_string(path).map_err(|err| {
        lock_error(
            "FUSE_LOCK_READ_FAILED",
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    let lock: Lockfile = toml::from_str(&content).map_err(|err| {
        lock_error(
            "FUSE_LOCK_PARSE_FAILED",
            format!("invalid lockfile {}: {err}", path.display()),
        )
    })?;
    if lock.version != 0 && lock.version != 1 {
        return Err(lock_error(
            "FUSE_LOCK_UNSUPPORTED_VERSION",
            format!(
                "unsupported lockfile version {} in {} (supported: 0, 1)",
                lock.version,
                path.display()
            ),
        ));
    }
    Ok(lock)
}

fn write_lockfile(path: &Path, lock: &Lockfile) -> Result<(), String> {
    let content = toml::to_string_pretty(lock).map_err(|err| {
        lock_error(
            "FUSE_LOCK_ENCODE_FAILED",
            format!("lockfile encode failed: {err}"),
        )
    })?;
    fs::write(path, content).map_err(|err| {
        lock_error(
            "FUSE_LOCK_WRITE_FAILED",
            format!("failed to write {}: {err}", path.display()),
        )
    })?;
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
