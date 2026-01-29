use std::env;
use std::fs;
use std::process;

use fusec::diag::Level;
use fusec::parse_source;

fn main() {
    let mut args = env::args().skip(1);
    let mut dump_ast = false;
    let mut check = false;
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
        if path.is_none() {
            path = Some(arg);
        } else {
            eprintln!("unexpected argument: {arg}");
            eprintln!("usage: fusec [--dump-ast] [--check] <file>");
            return;
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("usage: fusec [--dump-ast] [--check] <file>");
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

    if dump_ast {
        println!("{:#?}", program);
    }
}
