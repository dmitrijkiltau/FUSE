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
    changed_modules_since_meta, check_meta_files_unchanged, clean_build_dir, file_stamp,
    ir_meta_base_is_valid, ir_meta_is_valid, is_virtual_module_path, load_check_ir_meta,
    load_ir_meta, sha1_digest, write_check_ir_meta, write_ir_meta,
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
  --clean                 Remove .fuse/build before building (build only)
  --aot                   Emit deployable AOT binary (build only)
  --release               Use release profile for build output (build only; implies --aot)
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
    let common = match cli_args::parse_common_args(
        rest,
        allow_program_args,
        allow_clean,
        allow_build_mode,
        allow_test_filter,
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

    let deps = match deps::resolve_dependencies(manifest.as_ref(), manifest_dir.as_deref()) {
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
    };

    finalize_command(command, code)
}
