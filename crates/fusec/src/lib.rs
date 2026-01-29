pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;
pub mod sema;
pub mod span;
pub mod token;

use crate::diag::Diagnostics;

pub fn parse_source(src: &str) -> (ast::Program, Vec<diag::Diag>) {
    let mut diags = Diagnostics::default();
    let tokens = lexer::lex(src, &mut diags);
    let mut parser = parser::Parser::new(&tokens, &mut diags);
    let program = parser.parse_program();
    (program, diags.into_vec())
}
