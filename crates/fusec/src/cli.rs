use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use fuse_rt::error::{ValidationError, ValidationField};
use fuse_rt::json;

use crate::ast::{Item, TypeRefKind};
use crate::diag::Level;
use crate::interp::{Interpreter, MigrationJob, TestJob, TestOutcome};
use crate::{load_program_with_modules, load_program_with_modules_and_deps};

const USAGE: &str = "usage: fusec [--dump-ast] [--check] [--fmt] [--openapi] [--run] [--migrate] [--test] [--backend ast|vm] [--app NAME] <file>";

enum Backend {
    Ast,
    Vm,
}

pub fn run<I>(args: I) -> i32
where
    I: IntoIterator<Item = String>,
{
    run_with_deps(args, None)
}

pub fn run_with_deps<I>(args: I, deps: Option<&HashMap<String, PathBuf>>) -> i32
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut dump_ast = false;
    let mut check = false;
    let mut run = false;
    let mut fmt = false;
    let mut openapi = false;
    let mut program_args: Vec<String> = Vec::new();
    let mut backend = Backend::Ast;
    let mut backend_forced = false;
    let mut app_name: Option<String> = None;
    let mut migrate = false;
    let mut test = false;
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
        if arg == "--openapi" {
            openapi = true;
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
        if arg == "--test" {
            test = true;
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
                        eprintln!("{USAGE}");
                        return 1;
                    }
                };
            } else {
                eprintln!("--backend expects a name");
                eprintln!("{USAGE}");
                return 1;
            }
            continue;
        }
        if arg == "--app" {
            if let Some(name) = args.next() {
                app_name = Some(name);
            } else {
                eprintln!("--app expects a name");
                eprintln!("{USAGE}");
                return 1;
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
            eprintln!("{USAGE}");
            return 1;
        }
    };

    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("failed to read {path}: {err}");
            return 1;
        }
    };

    if fmt {
        let formatted = crate::format::format_source(&src);
        if formatted != src {
            if let Err(err) = fs::write(&path, formatted) {
                eprintln!("failed to write {path}: {err}");
                return 1;
            }
        }
        return 0;
    }

    let (registry, diags) = match deps {
        Some(deps) => load_program_with_modules_and_deps(Path::new(&path), &src, deps),
        None => load_program_with_modules(Path::new(&path), &src),
    };
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
        return 1;
    }
    let root = match registry.root() {
        Some(root) => root,
        None => {
            eprintln!("no root module loaded");
            return 1;
        }
    };
    let program = &root.program;

    if openapi {
        match crate::openapi::generate_openapi(&registry) {
            Ok(json) => {
                println!("{json}");
                return 0;
            }
            Err(err) => {
                eprintln!("openapi error: {err}");
                return 1;
            }
        }
    }

    if check {
        let (_analysis, diags) = crate::sema::analyze_registry(&registry);
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
            return 1;
        }
    }

    if migrate {
        let (_analysis, diags) = crate::sema::analyze_registry(&registry);
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
            return 1;
        }
        let migrations = match collect_migrations(&registry) {
            Ok(migrations) => migrations,
            Err(err) => {
                eprintln!("migration error: {err}");
                return 1;
            }
        };
        if !migrations.is_empty() {
            let mut interp = Interpreter::with_registry(&registry);
            if let Err(err) = interp.run_migrations(&migrations) {
                eprintln!("migration error: {err}");
                return 1;
            }
        }
        if !run {
            return 0;
        }
    }

    if test {
        let (_analysis, diags) = crate::sema::analyze_registry(&registry);
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
            return 1;
        }
        let tests = match collect_tests(&registry) {
            Ok(tests) => tests,
            Err(err) => {
                eprintln!("test error: {err}");
                return 1;
            }
        };
        let mut interp = Interpreter::with_registry(&registry);
        let outcomes = match interp.run_tests(&tests) {
            Ok(outcomes) => outcomes,
            Err(err) => {
                eprintln!("test error: {err}");
                return 1;
            }
        };
        let failed = report_tests(&outcomes);
        if failed > 0 {
            return 1;
        }
        if !run {
            return 0;
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
            let main_decl = program
                .items
                .iter()
                .find_map(|item| match item {
                    Item::Fn(decl) if decl.name.name == "main" => Some(decl),
                    _ => None,
                });
            let main_decl = match main_decl {
                Some(decl) => decl,
                None => {
                    eprintln!("no fn main found for CLI binding");
                    return 1;
                }
            };
            let raw = match parse_program_args(&program_args) {
                Ok(raw) => raw,
                Err(err) => {
                    emit_validation_error("$", "invalid_args", &err);
                    return 2;
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
                    args_map.insert(name.clone(), crate::interp::Value::Bool(*flag));
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
                return 2;
            }
            match backend {
                Backend::Ast => match interp.call_function_with_named_args("main", &args_map) {
                    Ok(_) => {}
                    Err(err) => {
                        emit_error_json(&err);
                        return 2;
                    }
                },
                Backend::Vm => {
                    let args = match interp.prepare_call_with_named_args("main", &args_map) {
                        Ok(args) => args,
                        Err(err) => {
                            emit_error_json(&err);
                            return 2;
                        }
                    };
                    let ir = match crate::ir::lower::lower_registry(&registry) {
                        Ok(ir) => ir,
                        Err(errors) => {
                            for error in errors {
                                eprintln!("lowering error: {error}");
                            }
                            return 1;
                        }
                    };
                    let mut vm = crate::vm::Vm::new(&ir);
                    match vm.call_function("main", args) {
                        Ok(_) => {}
                        Err(err) => {
                            emit_error_json(&err);
                            return 2;
                        }
                    }
                }
            }
        } else {
            match backend {
                Backend::Ast => {
                    let mut interp = Interpreter::with_registry(&registry);
                    if let Err(err) = interp.run_app(app) {
                        eprintln!("run error: {err}");
                        return 1;
                    }
                }
                Backend::Vm => {
                    let ir = match crate::ir::lower::lower_registry(&registry) {
                        Ok(ir) => ir,
                        Err(errors) => {
                            for error in errors {
                                eprintln!("lowering error: {error}");
                            }
                            return 1;
                        }
                    };
                    let mut vm = crate::vm::Vm::new(&ir);
                    if let Err(err) = vm.run_app(app) {
                        eprintln!("run error: {err}");
                        return 1;
                    }
                }
            }
        }
    }

    if dump_ast {
        println!("{:#?}", program);
    }

    0
}

fn collect_migrations<'a>(
    registry: &'a crate::ModuleRegistry,
) -> Result<Vec<MigrationJob<'a>>, String> {
    let mut jobs = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    for (id, unit) in &registry.modules {
        let module_path = unit.path.display().to_string();
        for item in &unit.program.items {
            if let Item::Migration(decl) = item {
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

fn collect_tests<'a>(registry: &'a crate::ModuleRegistry) -> Result<Vec<TestJob<'a>>, String> {
    let mut jobs = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    for (id, unit) in &registry.modules {
        let module_path = unit.path.display().to_string();
        for item in &unit.program.items {
            if let Item::Test(decl) = item {
                if decl.name.value.trim().is_empty() {
                    return Err("test name cannot be empty".to_string());
                }
                if let Some(prev) = seen.insert(decl.name.value.clone(), module_path.clone()) {
                    return Err(format!(
                        "duplicate test {} (also declared in {})",
                        decl.name.value, prev
                    ));
                }
                jobs.push((decl.name.value.clone(), module_path.clone(), *id, decl));
            }
        }
    }
    jobs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(jobs
        .into_iter()
        .map(|(name, _path, module_id, decl)| TestJob {
            name,
            module_id,
            decl,
        })
        .collect())
}

fn report_tests(outcomes: &[TestOutcome]) -> usize {
    if outcomes.is_empty() {
        println!("0 tests");
        return 0;
    }
    let mut failed = 0usize;
    for outcome in outcomes {
        if outcome.ok {
            println!("ok {}", outcome.name);
        } else {
            failed += 1;
            if let Some(message) = &outcome.message {
                println!("FAILED {}: {}", outcome.name, message);
            } else {
                println!("FAILED {}", outcome.name);
            }
        }
    }
    if failed == 0 {
        println!("ok ({} tests)", outcomes.len());
    } else {
        println!("FAILED ({} failed of {} tests)", failed, outcomes.len());
    }
    failed
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

fn is_optional(ty: &crate::ast::TypeRef) -> bool {
    match &ty.kind {
        TypeRefKind::Optional(_) => true,
        TypeRefKind::Generic { base, .. } => base.name == "Option",
        _ => false,
    }
}

fn is_bool_type(ty: &crate::ast::TypeRef) -> bool {
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
