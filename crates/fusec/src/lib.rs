pub mod ast;
pub mod callbind;
pub mod cli;
pub mod db;
pub mod diag;
pub mod format;
pub mod frontend;
pub mod html_tags;
pub mod interp;
pub mod ir;
pub mod lexer;
pub mod loader;
pub mod native;
pub mod openapi;
pub mod parser;
pub mod refinement;
mod runtime_assets;
mod runtime_svg;
pub mod runtime_types;
pub mod sema;
pub mod span;
mod task_pool;
pub mod token;
pub mod vm;

use crate::diag::Diagnostics;

pub use loader::{
    ModuleExports, ModuleId, ModuleLink, ModuleMap, ModuleRegistry, ModuleUnit, load_program,
    load_program_with_modules, load_program_with_modules_and_deps,
};

pub fn parse_source(src: &str) -> (ast::Program, Vec<diag::Diag>) {
    let mut diags = Diagnostics::default();
    let tokens = lexer::lex(src, &mut diags);
    let mut parser = parser::Parser::new(&tokens, &mut diags);
    let program = parser.parse_program();
    (program, diags.into_vec())
}
