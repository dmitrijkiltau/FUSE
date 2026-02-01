use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

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
  --backend <ast|vm>      Backend override (run only)
"#;

#[derive(Debug, Deserialize)]
struct Manifest {
    package: PackageConfig,
    #[serde(default)]
    build: Option<BuildConfig>,
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
}

#[derive(Default)]
struct CommonArgs {
    manifest_path: Option<PathBuf>,
    entry: Option<String>,
    app: Option<String>,
    backend: Option<String>,
    program_args: Vec<String>,
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
    let common = match parse_common_args(rest, allow_program_args) {
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

    let entry = resolve_entry(&common, manifest.as_ref(), manifest_dir.as_deref());
    let entry = match entry {
        Ok(entry) => entry,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    let app = common
        .app
        .clone()
        .or_else(|| manifest.as_ref().and_then(|m| m.package.app.clone()));
    let backend = common
        .backend
        .clone()
        .or_else(|| manifest.as_ref().and_then(|m| m.package.backend.clone()));
    if let Some(backend) = &backend {
        if backend != "ast" && backend != "vm" {
            eprintln!("unknown backend: {backend}");
            return 1;
        }
    }

    match command {
        Command::Run => {
            let mut args = Vec::new();
            args.push("--run".to_string());
            if let Some(backend) = backend {
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
            fusec::cli::run(args)
        }
        Command::Test => {
            let mut args = Vec::new();
            args.push("--test".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run(args)
        }
        Command::Build => run_build(&entry, manifest.as_ref(), manifest_dir.as_deref()),
        Command::Check => {
            let mut args = Vec::new();
            args.push("--check".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run(args)
        }
        Command::Fmt => {
            let mut args = Vec::new();
            args.push("--fmt".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run(args)
        }
        Command::Openapi => {
            let mut args = Vec::new();
            args.push("--openapi".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run(args)
        }
        Command::Migrate => {
            let mut args = Vec::new();
            args.push("--migrate".to_string());
            args.push(entry.to_string_lossy().to_string());
            fusec::cli::run(args)
        }
    }
}

fn parse_common_args(args: &[String], allow_program_args: bool) -> Result<CommonArgs, String> {
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
        if arg.starts_with("--") {
            return Err(format!("unknown option: {arg}"));
        }
        if out.entry.is_none() {
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
            (Some(path.to_path_buf()), path.parent().map(|p| p.to_path_buf()))
        }
    } else {
        let cwd = env::current_dir().map_err(|err| format!("cwd error: {err}"))?;
        let path = find_manifest(&cwd);
        let dir = path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf()));
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
        return Err("missing entry: pass a file path or set package.entry in fuse.toml".to_string());
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

fn run_build(entry: &Path, manifest: Option<&Manifest>, manifest_dir: Option<&Path>) -> i32 {
    let mut check_args = Vec::new();
    check_args.push("--check".to_string());
    check_args.push(entry.to_string_lossy().to_string());
    let code = fusec::cli::run(check_args);
    if code != 0 {
        return code;
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
    if let Err(err) = write_openapi(entry, &out_path) {
        eprintln!("{err}");
        return 1;
    }
    0
}

fn write_openapi(entry: &Path, out_path: &Path) -> Result<(), String> {
    let src = fs::read_to_string(entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let (registry, diags) = fusec::load_program_with_modules(entry, &src);
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
