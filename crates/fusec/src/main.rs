use std::env;
use std::fs;
use std::process;

use fusec::diag::Level;
use fusec::parse_source;

enum Backend {
    Ast,
    Vm,
}

fn main() {
    let mut args = env::args().skip(1);
    let mut dump_ast = false;
    let mut check = false;
    let mut run = false;
    let mut backend = Backend::Ast;
    let mut app_name: Option<String> = None;
    let mut path = None;

    while let Some(arg) = args.next() {
        if arg == "--dump-ast" {
            dump_ast = true;
            continue;
        }
        if arg == "--check" {
            check = true;
            continue;
        }
        if arg == "--run" {
            run = true;
            continue;
        }
        if arg == "--backend" {
            if let Some(name) = args.next() {
                backend = match name.as_str() {
                    "ast" => Backend::Ast,
                    "vm" => Backend::Vm,
                    _ => {
                        eprintln!("unknown backend: {name}");
                        eprintln!("usage: fusec [--dump-ast] [--check] [--run] [--backend ast|vm] [--app NAME] <file>");
                        return;
                    }
                };
            } else {
                eprintln!("--backend expects a name");
                eprintln!("usage: fusec [--dump-ast] [--check] [--run] [--backend ast|vm] [--app NAME] <file>");
                return;
            }
            continue;
        }
        if arg == "--app" {
            if let Some(name) = args.next() {
                app_name = Some(name);
            } else {
                eprintln!("--app expects a name");
                eprintln!("usage: fusec [--dump-ast] [--check] [--run] [--backend ast|vm] [--app NAME] <file>");
                return;
            }
            continue;
        }
        if path.is_none() {
            path = Some(arg);
        } else {
            eprintln!("unexpected argument: {arg}");
            eprintln!("usage: fusec [--dump-ast] [--check] [--run] [--backend ast|vm] [--app NAME] <file>");
            return;
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("usage: fusec [--dump-ast] [--check] [--run] [--backend ast|vm] [--app NAME] <file>");
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

    let (program, diags) = parse_source(&src);
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

    if check {
        let (_analysis, diags) = fusec::sema::analyze_program(&program);
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

    if run {
        let app = app_name.as_deref();
        match backend {
            Backend::Ast => {
                let mut interp = fusec::interp::Interpreter::new(&program);
                if let Err(err) = interp.run_app(app) {
                    eprintln!("run error: {err}");
                    process::exit(1);
                }
            }
            Backend::Vm => {
                let ir = match fusec::ir::lower::lower_program(&program) {
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

    if dump_ast {
        println!("{:#?}", program);
    }
}
