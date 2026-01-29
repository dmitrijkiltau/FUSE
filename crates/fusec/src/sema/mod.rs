pub mod check;
pub mod symbols;
pub mod types;

use crate::ast::Program;
use crate::diag::{Diag, Diagnostics};

pub struct Analysis {
    pub symbols: symbols::ModuleSymbols,
}

pub fn analyze_program(program: &Program) -> (Analysis, Vec<Diag>) {
    let mut diags = Diagnostics::default();
    let symbols = symbols::collect(program, &mut diags);
    let mut checker = check::Checker::new(&symbols, &mut diags);
    checker.check_program(program);
    (Analysis { symbols }, diags.into_vec())
}
