pub mod ast;
pub mod cli;
pub mod db;
pub mod diag;
pub mod format;
pub mod interp;
pub mod ir;
pub mod lexer;
pub mod loader;
pub mod native;
pub mod openapi;
pub mod parser;
mod runtime_assets;
pub mod sema;
pub mod span;
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
