use std::env;
use std::path::PathBuf;

mod aot;
mod assets;
mod cache;
mod cli_args;
mod cli_output;
mod command_ops;
mod deps;
mod dev;
mod manifest;
mod model;
mod runtime_env;

pub(crate) use cache::{
    FileStamp, affected_modules_for_incremental_check, build_dir, build_ir_meta,
    changed_modules_since_meta, check_meta_files_unchanged, clean_build_dir, clean_fuse_cache_dirs,
    file_stamp, ir_meta_base_is_valid, ir_meta_is_valid, is_virtual_module_path,
    load_check_ir_meta, load_ir_meta, sha1_digest, write_check_ir_meta, write_ir_meta,
};
pub(crate) use cli_output::{
    apply_color_choice, apply_diagnostics_format, dev_prefix, emit_aot_build_progress,
    emit_cli_error, emit_cli_warning, emit_command_step, emit_diags, emit_diags_with_fallback,
    emit_usage, finalize_command, line_info, style_error,
};
pub(crate) use model::{
    DependencyDetail, DependencySpec, IrFileMeta, IrMeta, Manifest, ServeConfig,
};

const USAGE: &str = r#"usage: fuse <command> [options] [file] [-- <program args>]

commands:
  dev       Run package entrypoint with live reload (watch mode)
  run       Run the package entrypoint
  test      Run tests in the package
  build     Run package checks (and optional build steps)
  check     Parse + sema check
  clean     Remove selected cache directories
  deps      Dependency maintenance commands
  fmt       Format a Fuse file
  openapi   Emit OpenAPI JSON
  migrate   Run database migrations

options:
  --manifest-path <path>  Path to fuse.toml (defaults to nearest parent)
  --file <path>           Entry file override
  --app <name>            App name override
  --filter <pattern>      Run only tests matching substring pattern (test only)
  --backend <ast|native>  Backend override (run only)
  --strict-architecture   Enable strict architectural checks during semantic analysis
  --diagnostics <json|text>  Emit structured JSON diagnostics or force text mode
  --color <auto|always|never>  Colorized CLI output policy
  --frozen                Refuse fuse.lock mutation (check/run/build/test only)
  --clean                 Remove .fuse/build before building (build only)
  --cache                 Remove .fuse-cache directories under a selected root (clean only)
  --aot                   Emit deployable AOT binary (build only)
  --release               Use release profile for build output (build only; implies --aot)

dependency commands:
  deps lock [--check|--update] [--manifest-path <path>]
                        Refresh fuse.lock or fail if it is out of date
  deps publish-check [<path>|--manifest-path <path>]
                        Check workspace manifest/lock readiness for publish
  clean --cache [<path>|--manifest-path <path>]
                        Remove .fuse-cache directories under the selected root
"#;

const FUSE_ASSET_MAP_ENV: &str = "FUSE_ASSET_MAP";
const BUILD_TARGET_FINGERPRINT: &str = env!("FUSE_BUILD_TARGET");
const BUILD_RUSTC_FINGERPRINT: &str = env!("FUSE_BUILD_RUSTC_VERSION");
const BUILD_CLI_VERSION_FINGERPRINT: &str = env!("CARGO_PKG_VERSION");
const AOT_SEMANTIC_CONTRACT_VERSION: &str = "aot-v1";
const AOT_BUILD_PROGRESS_STAGES: usize = 6;

#[derive(Default)]
struct CommonArgs {
    manifest_path: Option<PathBuf>,
    entry: Option<String>,
    app: Option<String>,
    backend: Option<String>,
    test_filter: Option<String>,
    diagnostics: Option<DiagnosticsFormat>,
    color: Option<ColorChoice>,
    program_args: Vec<String>,
    clean: bool,
    aot: bool,
    release: bool,
    strict_architecture: bool,
    frozen: bool,
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum DepsLockMode {
    Update,
    Check,
}

struct DepsCommonArgs {
    path: Option<PathBuf>,
    diagnostics: Option<DiagnosticsFormat>,
    color: Option<ColorChoice>,
}

struct DepsLockArgs {
    common: DepsCommonArgs,
    mode: DepsLockMode,
}

struct CleanArgs {
    path: Option<PathBuf>,
    diagnostics: Option<DiagnosticsFormat>,
    color: Option<ColorChoice>,
    cache: bool,
}

#[derive(Copy, Clone)]
enum Command {
    Dev,
    Run,
    Test,
    Build,
    Check,
    Clean,
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

#[derive(Copy, Clone, Eq, PartialEq)]
enum DiagnosticsFormat {
    Text,
    Json,
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

impl DiagnosticsFormat {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "json" => Some(Self::Json),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    fn as_env_value(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
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
    apply_diagnostics_format(DiagnosticsFormat::Text);
    if args.is_empty() {
        emit_usage();
        return 1;
    }
    let (cmd, rest) = args.split_first().unwrap();
    if let Some(format) = cli_args::discover_diagnostics_format(rest) {
        apply_diagnostics_format(format);
    }
    if cmd == "deps" {
        if let Some(choice) = cli_args::discover_color_choice(rest) {
            apply_color_choice(choice);
        }
        return run_deps_command(rest);
    }
    if cmd == "clean" {
        if let Some(choice) = cli_args::discover_color_choice(rest) {
            apply_color_choice(choice);
        }
        return run_clean_command(rest);
    }
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
            emit_usage();
            return 1;
        }
    };
    if let Some(choice) = cli_args::discover_color_choice(rest) {
        apply_color_choice(choice);
    }
    let allow_program_args = matches!(command, Command::Run);
    let allow_clean = matches!(command, Command::Build);
    let allow_build_mode = matches!(command, Command::Build);
    let allow_test_filter = matches!(command, Command::Test);
    let allow_frozen = matches!(
        command,
        Command::Run | Command::Test | Command::Build | Command::Check
    );
    let common = match cli_args::parse_common_args(
        rest,
        allow_program_args,
        allow_clean,
        allow_build_mode,
        allow_test_filter,
        allow_frozen,
    ) {
        Ok(args) => args,
        Err(err) => {
            emit_cli_error(&err);
            emit_usage();
            return 1;
        }
    };
    apply_diagnostics_format(common.diagnostics.unwrap_or(DiagnosticsFormat::Text));
    apply_color_choice(common.color.unwrap_or(ColorChoice::Auto));

    let (manifest, manifest_dir) = match manifest::load_manifest(common.manifest_path.as_deref()) {
        Ok(value) => value,
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };
    runtime_env::apply_dotenv(manifest_dir.as_deref());
    runtime_env::apply_default_config_path(manifest_dir.as_deref());

    let entry = manifest::resolve_entry(&common, manifest.as_ref(), manifest_dir.as_deref());
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

    let lock_mode = if common.frozen {
        deps::LockMode::Frozen
    } else {
        deps::LockMode::Update
    };
    let deps = match deps::resolve_dependencies_with_options(
        manifest.as_ref(),
        manifest_dir.as_deref(),
        deps::ResolveOptions { lock_mode },
    ) {
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
        if let Err(err) = runtime_env::configure_openapi_ui_env(
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
                if let Some(native) = aot::try_load_native(manifest_dir.as_deref()) {
                    runtime_env::apply_serve_env(manifest.as_ref(), manifest_dir.as_deref());
                    return finalize_command(
                        command,
                        aot::run_native_program(
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
        Command::Dev => dev::run_dev(
            &entry,
            manifest.as_ref(),
            manifest_dir.as_deref(),
            &deps,
            app.as_deref(),
            backend,
            common.strict_architecture,
        ),
        Command::Run => {
            runtime_env::apply_serve_env(manifest.as_ref(), manifest_dir.as_deref());
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
            if let Some(filter) = common.test_filter {
                args.push("--filter".to_string());
                args.push(filter);
            }
            if common.strict_architecture {
                args.push("--strict-architecture".to_string());
            }
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run_with_deps(args, Some(&deps))
        }
        Command::Build => command_ops::run_build(
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
                command_ops::run_project_check(
                    &entry,
                    manifest_dir.as_deref(),
                    &deps,
                    common.strict_architecture,
                )
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
                command_ops::run_project_fmt(&entry, &deps)
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
        Command::Clean => unreachable!("clean is handled before manifest loading"),
    };

    finalize_command(command, code)
}

fn run_clean_command(args: &[String]) -> i32 {
    let parsed = match parse_clean_args(args) {
        Ok(args) => args,
        Err(err) => {
            emit_cli_error(&err);
            emit_usage();
            return 1;
        }
    };
    apply_diagnostics_format(parsed.diagnostics.unwrap_or(DiagnosticsFormat::Text));
    apply_color_choice(parsed.color.unwrap_or(ColorChoice::Auto));
    emit_command_step(Command::Clean, "start");

    let root = match resolve_clean_root(parsed.path) {
        Ok(root) => root,
        Err(err) => {
            emit_cli_error(&err);
            return finalize_command(Command::Clean, 1);
        }
    };
    let removed = match clean_fuse_cache_dirs(&root) {
        Ok(removed) => removed,
        Err(err) => {
            emit_cli_error(&err);
            return finalize_command(Command::Clean, 1);
        }
    };
    if removed == 0 {
        emit_command_step(
            Command::Clean,
            &format!("no .fuse-cache directories found under {}", root.display()),
        );
    } else {
        emit_command_step(
            Command::Clean,
            &format!(
                "removed {removed} .fuse-cache director{} under {}",
                if removed == 1 { "y" } else { "ies" },
                root.display()
            ),
        );
    }
    finalize_command(Command::Clean, 0)
}

fn run_deps_command(args: &[String]) -> i32 {
    let mut subcmd_idx = 0usize;
    while subcmd_idx < args.len() {
        let arg = &args[subcmd_idx];
        match arg.as_str() {
            "--diagnostics" | "--color" => {
                subcmd_idx += 2;
            }
            _ if arg.starts_with("--diagnostics=") || arg.starts_with("--color=") => {
                subcmd_idx += 1;
            }
            _ => break,
        }
    }
    if subcmd_idx >= args.len() {
        emit_cli_error("missing deps subcommand");
        emit_usage();
        return 1;
    }
    let subcmd = &args[subcmd_idx];
    let mut rest: Vec<String> = args[..subcmd_idx].to_vec();
    rest.extend_from_slice(&args[subcmd_idx + 1..]);
    match subcmd.as_str() {
        "lock" => run_deps_lock_command(&rest),
        "publish-check" => run_deps_publish_check_command(&rest),
        _ => {
            emit_cli_error(&format!("unknown deps subcommand: {subcmd}"));
            emit_usage();
            1
        }
    }
}

fn run_deps_lock_command(args: &[String]) -> i32 {
    let parsed = match parse_deps_lock_args(args) {
        Ok(args) => args,
        Err(err) => {
            emit_cli_error(&err);
            emit_usage();
            return 1;
        }
    };
    apply_diagnostics_format(parsed.common.diagnostics.unwrap_or(DiagnosticsFormat::Text));
    apply_color_choice(parsed.common.color.unwrap_or(ColorChoice::Auto));

    let (manifest, manifest_dir) = match manifest::load_manifest(parsed.common.path.as_deref()) {
        Ok((Some(manifest), Some(dir))) => (manifest, dir),
        Ok((Some(_), None)) => {
            emit_cli_error("dependencies require a manifest directory");
            return 1;
        }
        Ok((None, _)) => {
            emit_cli_error(
                "missing manifest: pass --manifest-path <path> or run from a package directory",
            );
            return 1;
        }
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };
    let lock_mode = match parsed.mode {
        DepsLockMode::Update => deps::LockMode::Update,
        DepsLockMode::Check => deps::LockMode::Check,
    };
    match deps::resolve_dependencies_with_options(
        Some(&manifest),
        Some(&manifest_dir),
        deps::ResolveOptions { lock_mode },
    ) {
        Ok(_) => 0,
        Err(err) => {
            emit_cli_error(&err);
            1
        }
    }
}

fn run_deps_publish_check_command(args: &[String]) -> i32 {
    let parsed = match parse_deps_common_args(args, "publish-check") {
        Ok(args) => args,
        Err(err) => {
            emit_cli_error(&err);
            emit_usage();
            return 1;
        }
    };
    apply_diagnostics_format(parsed.diagnostics.unwrap_or(DiagnosticsFormat::Text));
    apply_color_choice(parsed.color.unwrap_or(ColorChoice::Auto));

    let root = match resolve_publish_check_root(parsed.path) {
        Ok(root) => root,
        Err(err) => {
            emit_cli_error(&err);
            return 1;
        }
    };
    match deps::check_workspace_publish_readiness(&root) {
        Ok(()) => 0,
        Err(err) => {
            emit_cli_error(&err);
            1
        }
    }
}

fn parse_clean_args(args: &[String]) -> Result<CleanArgs, String> {
    let mut out = CleanArgs {
        path: None,
        diagnostics: None,
        color: None,
        cache: false,
    };
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--manifest-path" {
            idx += 1;
            let Some(path) = args.get(idx) else {
                return Err("--manifest-path expects a path".to_string());
            };
            if out.path.is_some() {
                return Err("clean target path already set".to_string());
            }
            out.path = Some(PathBuf::from(path));
            idx += 1;
            continue;
        }
        if arg == "--diagnostics" {
            idx += 1;
            let Some(mode) = args.get(idx) else {
                return Err("--diagnostics expects json|text".to_string());
            };
            let Some(parsed) = DiagnosticsFormat::parse(mode) else {
                return Err(format!(
                    "invalid --diagnostics value: {mode} (expected json|text)"
                ));
            };
            out.diagnostics = Some(parsed);
            idx += 1;
            continue;
        }
        if let Some(mode) = arg.strip_prefix("--diagnostics=") {
            let Some(parsed) = DiagnosticsFormat::parse(mode) else {
                return Err(format!(
                    "invalid --diagnostics value: {mode} (expected json|text)"
                ));
            };
            out.diagnostics = Some(parsed);
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
        if arg == "--cache" {
            out.cache = true;
            idx += 1;
            continue;
        }
        if arg == "--frozen"
            || arg == "--clean"
            || arg == "--aot"
            || arg == "--release"
            || arg == "--strict-architecture"
        {
            return Err(format!("{arg} is not supported for fuse clean"));
        }
        if arg == "--file" || arg == "--app" || arg == "--backend" || arg == "--filter" {
            return Err(format!("{arg} is not supported for fuse clean"));
        }
        if arg.starts_with("--") {
            return Err(format!("unknown option: {arg}"));
        }
        if out.path.is_none() {
            out.path = Some(PathBuf::from(arg));
            idx += 1;
            continue;
        }
        return Err(format!("unexpected argument: {arg}"));
    }
    if !out.cache {
        return Err("fuse clean requires --cache".to_string());
    }
    Ok(out)
}

fn parse_deps_lock_args(args: &[String]) -> Result<DepsLockArgs, String> {
    let mut common = parse_deps_common_args(args, "lock")?;
    let mut mode = DepsLockMode::Update;
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = &args[idx];
        match arg.as_str() {
            "--manifest-path" | "--diagnostics" | "--color" => {
                idx += 2;
                continue;
            }
            _ if arg.starts_with("--diagnostics=") || arg.starts_with("--color=") => {
                idx += 1;
                continue;
            }
            "--check" => {
                mode = DepsLockMode::Check;
                idx += 1;
                continue;
            }
            "--update" => {
                mode = DepsLockMode::Update;
                idx += 1;
                continue;
            }
            _ if arg.starts_with("--") => {
                idx += 1;
                continue;
            }
            _ => {
                if common.path.is_none() {
                    common.path = Some(PathBuf::from(arg));
                }
                idx += 1;
            }
        }
    }
    Ok(DepsLockArgs { common, mode })
}

fn parse_deps_common_args(args: &[String], subcommand: &str) -> Result<DepsCommonArgs, String> {
    let mut out = DepsCommonArgs {
        path: None,
        diagnostics: None,
        color: None,
    };
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--manifest-path" {
            idx += 1;
            let Some(path) = args.get(idx) else {
                return Err("--manifest-path expects a path".to_string());
            };
            out.path = Some(PathBuf::from(path));
            idx += 1;
            continue;
        }
        if arg == "--diagnostics" {
            idx += 1;
            let Some(mode) = args.get(idx) else {
                return Err("--diagnostics expects json|text".to_string());
            };
            let Some(parsed) = DiagnosticsFormat::parse(mode) else {
                return Err(format!(
                    "invalid --diagnostics value: {mode} (expected json|text)"
                ));
            };
            out.diagnostics = Some(parsed);
            idx += 1;
            continue;
        }
        if let Some(mode) = arg.strip_prefix("--diagnostics=") {
            let Some(parsed) = DiagnosticsFormat::parse(mode) else {
                return Err(format!(
                    "invalid --diagnostics value: {mode} (expected json|text)"
                ));
            };
            out.diagnostics = Some(parsed);
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
        if arg == "--check" || arg == "--update" {
            if subcommand == "lock" {
                idx += 1;
                continue;
            }
            return Err(format!("{arg} is not supported for fuse deps {subcommand}"));
        }
        if arg == "--frozen"
            || arg == "--clean"
            || arg == "--aot"
            || arg == "--release"
            || arg == "--strict-architecture"
        {
            return Err(format!("{arg} is not supported for fuse deps {subcommand}"));
        }
        if arg == "--file" || arg == "--app" || arg == "--backend" || arg == "--filter" {
            return Err(format!("{arg} is not supported for fuse deps {subcommand}"));
        }
        if arg.starts_with("--") {
            return Err(format!("unknown option: {arg}"));
        }
        if out.path.is_none() {
            out.path = Some(PathBuf::from(arg));
            idx += 1;
            continue;
        }
        return Err(format!("unexpected argument: {arg}"));
    }
    Ok(out)
}

fn resolve_clean_root(path: Option<PathBuf>) -> Result<PathBuf, String> {
    let path = match path {
        Some(path) => path,
        None => env::current_dir().map_err(|err| format!("cwd error: {err}"))?,
    };
    if path.is_dir() {
        return Ok(path);
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "fuse.toml")
    {
        return path
            .parent()
            .map(|parent| parent.to_path_buf())
            .ok_or_else(|| {
                format!(
                    "clean root error: cannot resolve parent for {}",
                    path.display()
                )
            });
    }
    Err(format!(
        "clean root must be a directory or fuse.toml path, got {}",
        path.display()
    ))
}

fn resolve_publish_check_root(path: Option<PathBuf>) -> Result<PathBuf, String> {
    let path = match path {
        Some(path) => path,
        None => env::current_dir().map_err(|err| format!("cwd error: {err}"))?,
    };
    if path.is_dir() {
        return Ok(path);
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "fuse.toml")
    {
        return path
            .parent()
            .map(|parent| parent.to_path_buf())
            .ok_or_else(|| {
                format!(
                    "workspace root error: cannot resolve parent for {}",
                    path.display()
                )
            });
    }
    Err(format!(
        "workspace root must be a directory or fuse.toml path, got {}",
        path.display()
    ))
}
