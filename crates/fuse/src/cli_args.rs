use std::path::PathBuf;

use super::{ColorChoice, CommonArgs, DiagnosticsFormat};

pub fn discover_color_choice(args: &[String]) -> Option<ColorChoice> {
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

pub fn discover_diagnostics_format(args: &[String]) -> Option<DiagnosticsFormat> {
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            break;
        }
        if arg == "--diagnostics" {
            idx += 1;
            let value = args.get(idx)?;
            return DiagnosticsFormat::parse(value);
        }
        if let Some(value) = arg.strip_prefix("--diagnostics=") {
            return DiagnosticsFormat::parse(value);
        }
        idx += 1;
    }
    None
}

pub fn parse_common_args(
    args: &[String],
    allow_program_args: bool,
    allow_clean: bool,
    allow_build_mode: bool,
    allow_test_filter: bool,
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
        if arg == "--filter" {
            if !allow_test_filter {
                return Err("--filter is only supported for fuse test".to_string());
            }
            idx += 1;
            let Some(pattern) = args.get(idx) else {
                return Err("--filter expects a pattern".to_string());
            };
            out.test_filter = Some(pattern.clone());
            idx += 1;
            continue;
        }
        if let Some(pattern) = arg.strip_prefix("--filter=") {
            if !allow_test_filter {
                return Err("--filter is only supported for fuse test".to_string());
            }
            out.test_filter = Some(pattern.to_string());
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
