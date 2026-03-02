use std::io;

#[path = "fuse_lsp/mod.rs"]
mod lsp;
pub(crate) use lsp::core::*;
pub(crate) use lsp::symbols::*;
pub(crate) use lsp::workspace::*;

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut state = LspState::default();
    lsp::server::run(&mut stdin, &mut stdout, &mut state)
}
