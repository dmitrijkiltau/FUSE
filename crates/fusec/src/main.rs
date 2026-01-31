use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::Path;
use std::process;

use fusec::diag::Level;
use fusec::load_program_with_modules;
use fusec::interp::{Interpreter, MigrationJob};
use fusec::ast::TypeRefKind;
use fuse_rt::error::{ValidationError, ValidationField};
use fuse_rt::json;

enum Backend {
    Ast,
    Vm,
}

fn main() {
    let mut args = env::args().skip(1);
    let mut dump_ast = false;
    let mut check = false;
    let mut run = false;
    let mut fmt = false;
    let mut program_args: Vec<String> = Vec::new();
    let mut backend = Backend::Ast;
    let mut backend_forced = false;
    let mut app_name: Option<String> = None;
    let mut migrate = false;
    let mut path = None;

    while let Some(arg) = args.next() {
        if arg == "--" {
            program_args.extend(args);
            break;
        }
        if arg == "--dump-ast" {
            dump_ast = true;
            continue;
        }
        if arg == "--check" {
            check = true;
            continue;
        }
        if arg == "--fmt" {
            fmt = true;
            continue;
        }
        if arg == "--run" {
            run = true;
            continue;
        }
        if arg == "--migrate" {
            migrate = true;
            continue;
        }
        if arg == "--backend" {
            if let Some(name) = args.next() {
                backend_forced = true;
                backend = match name.as_str() {
                    "ast" => Backend::Ast,
                    "vm" => Backend::Vm,
                    _ => {
                        eprintln!("unknown backend: {name}");
                        eprintln!("usage: fusec [--dump-ast] [--check] [--fmt] [--run] [--migrate] [--backend ast|vm] [--app NAME] <file>");
                        return;
                    }
                };
            } else {
                eprintln!("--backend expects a name");
                eprintln!("usage: fusec [--dump-ast] [--check] [--fmt] [--run] [--migrate] [--backend ast|vm] [--app NAME] <file>");
                return;
            }
            continue;
        }
        if arg == "--app" {
            if let Some(name) = args.next() {
                app_name = Some(name);
            } else {
                eprintln!("--app expects a name");
                eprintln!("usage: fusec [--dump-ast] [--check] [--fmt] [--run] [--migrate] [--backend ast|vm] [--app NAME] <file>");
                return;
            }
            continue;
        }
        if path.is_none() {
            path = Some(arg);
        } else {
            program_args.push(arg);
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("usage: fusec [--dump-ast] [--check] [--fmt] [--run] [--migrate] [--backend ast|vm] [--app NAME] <file>");
            return;
        }
    };

    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("failed to read {path}: {err}");
            process::exit(1);
        }
    };

    if fmt {
        let formatted = fusec::format::format_source(&src);
        if formatted != src {
            if let Err(err) = fs::write(&path, formatted) {
                eprintln!("failed to write {path}: {err}");
                process::exit(1);
            }
        }
        return;
    }

    let (registry, diags) = load_program_with_modules(Path::new(&path), &src);
    if !diags.is_empty() {
        for diag in diags {
            let level = match diag.level {
                Level::Error => "error",
                Level::Warning => "warning",
            };
            eprintln!(
                "{level}: {} ({}..{})",
                diag.message, diag.span.start, diag.span.end
            );
        }
        process::exit(1);
    }
    let root = match registry.root() {
        Some(root) => root,
        None => {
            eprintln!("no root module loaded");
            process::exit(1);
        }
    };
    let program = &root.program;

    if check {
        let (_analysis, diags) = fusec::sema::analyze_registry(&registry);
        if !diags.is_empty() {
            for diag in diags {
                let level = match diag.level {
                    Level::Error => "error",
                    Level::Warning => "warning",
                };
                eprintln!(
                    "{level}: {} ({}..{})",
                    diag.message, diag.span.start, diag.span.end
                );
            }
            process::exit(1);
        }
    }

    if migrate {
        let (_analysis, diags) = fusec::sema::analyze_registry(&registry);
        if !diags.is_empty() {
            for diag in diags {
                let level = match diag.level {
                    Level::Error => "error",
                    Level::Warning => "warning",
                };
                eprintln!(
                    "{level}: {} ({}..{})",
                    diag.message, diag.span.start, diag.span.end
                );
            }
            process::exit(1);
        }
        let migrations = match collect_migrations(&registry) {
            Ok(migrations) => migrations,
            Err(err) => {
                eprintln!("migration error: {err}");
                process::exit(1);
            }
        };
        if !migrations.is_empty() {
            let mut interp = Interpreter::with_registry(&registry);
            if let Err(err) = interp.run_migrations(&migrations) {
                eprintln!("migration error: {err}");
                process::exit(1);
            }
        }
        if !run {
            return;
        }
    }

    if run {
        if !backend_forced {
            backend = if !program_args.is_empty() {
                Backend::Ast
            } else {
                Backend::Vm
            };
        }
        let app = app_name.as_deref();
        if !program_args.is_empty() {
            if !matches!(backend, Backend::Ast) {
                eprintln!("CLI binding is only supported on --backend ast for now");
                process::exit(1);
            }
            let main_decl = program
                .items
                .iter()
                .find_map(|item| match item {
                    fusec::ast::Item::Fn(decl) if decl.name.name == "main" => Some(decl),
                    _ => None,
                });
            let main_decl = match main_decl {
                Some(decl) => decl,
                None => {
                    eprintln!("no fn main found for CLI binding");
                    process::exit(1);
                }
            };
            let raw = match parse_program_args(&program_args) {
                Ok(raw) => raw,
                Err(err) => {
                    emit_validation_error("$", "invalid_args", &err);
                    return;
                }
            };
            let mut interp = Interpreter::with_registry(&registry);
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
                process::exit(2);
            }
            match interp.call_function_with_named_args("main", &args_map) {
                Ok(_) => {}
                Err(err) => {
                    emit_error_json(&err);
                    process::exit(2);
                }
            }
        } else {
            match backend {
                Backend::Ast => {
                    let mut interp = fusec::interp::Interpreter::with_registry(&registry);
                    if let Err(err) = interp.run_app(app) {
                        eprintln!("run error: {err}");
                        process::exit(1);
                    }
                }
                Backend::Vm => {
                    let ir = match fusec::ir::lower::lower_registry(&registry) {
                        Ok(ir) => ir,
                        Err(errors) => {
                            for error in errors {
                                eprintln!("lowering error: {error}");
                            }
                            process::exit(1);
                        }
                    };
                    let mut vm = fusec::vm::Vm::new(&ir);
                    if let Err(err) = vm.run_app(app) {
                        eprintln!("run error: {err}");
                        process::exit(1);
                    }
                }
            }
        }
    }

    if dump_ast {
        println!("{:#?}", program);
    }
}

fn collect_migrations<'a>(
    registry: &'a fusec::ModuleRegistry,
) -> Result<Vec<MigrationJob<'a>>, String> {
    let mut jobs = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    for (id, unit) in &registry.modules {
        let module_path = unit.path.display().to_string();
        for item in &unit.program.items {
            if let fusec::ast::Item::Migration(decl) = item {
                if decl.name.trim().is_empty() {
                    return Err("migration name cannot be empty".to_string());
                }
                if let Some(prev) = seen.insert(decl.name.clone(), module_path.clone()) {
                    return Err(format!(
                        "duplicate migration {} (also declared in {})",
                        decl.name, prev
                    ));
                }
                jobs.push((decl.name.clone(), module_path.clone(), *id, decl));
            }
        }
    }
    jobs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(jobs
        .into_iter()
        .map(|(id, _path, module_id, decl)| MigrationJob {
            id,
            module_id,
            decl,
        })
        .collect())
}

struct RawArgs {
    values: HashMap<String, Vec<String>>,
    bools: HashMap<String, bool>,
}

fn parse_program_args(args: &[String]) -> Result<RawArgs, String> {
    let mut values: HashMap<String, Vec<String>> = HashMap::new();
    let mut bools: HashMap<String, bool> = HashMap::new();
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if !arg.starts_with("--") {
            return Err(format!("unexpected argument: {arg}"));
        }
        if let Some((name, val)) = arg.strip_prefix("--").and_then(|s| s.split_once('=')) {
            values.entry(name.to_string()).or_default().push(val.to_string());
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
    Ok(RawArgs { values, bools })
}

fn is_optional(ty: &fusec::ast::TypeRef) -> bool {
    match &ty.kind {
        TypeRefKind::Optional(_) => true,
        TypeRefKind::Generic { base, .. } => base.name == "Option",
        _ => false,
    }
}

fn is_bool_type(ty: &fusec::ast::TypeRef) -> bool {
    match &ty.kind {
        TypeRefKind::Simple(ident) => ident.name == "Bool",
        TypeRefKind::Refined { base, .. } => base.name == "Bool",
        TypeRefKind::Optional(inner) => is_bool_type(inner),
        TypeRefKind::Generic { base, args } => {
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
    let json_value = err.to_json();
    eprintln!("{}", json::encode(&json_value));
}

fn emit_error_json(message: &str) {
    if message.trim_start().starts_with('{') {
        eprintln!("{message}");
        return;
    }
    emit_validation_error("$", "runtime_error", message);
}
