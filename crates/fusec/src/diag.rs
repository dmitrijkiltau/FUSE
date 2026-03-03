use std::path::PathBuf;

use crate::span::Span;

#[derive(Clone, Debug)]
pub enum Level {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Diag {
    pub level: Level,
    pub code: Option<String>,
    pub message: String,
    pub span: Span,
    pub path: Option<PathBuf>,
}

#[derive(Default, Debug)]
pub struct Diagnostics {
    diags: Vec<Diag>,
}

impl Diagnostics {
    pub fn error<S: Into<String>>(&mut self, span: Span, message: S) {
        self.diags.push(Diag {
            level: Level::Error,
            code: None,
            message: message.into(),
            span,
            path: None,
        });
    }

    pub fn error_with_code<C, S>(&mut self, span: Span, code: C, message: S)
    where
        C: Into<String>,
        S: Into<String>,
    {
        self.diags.push(Diag {
            level: Level::Error,
            code: Some(code.into()),
            message: message.into(),
            span,
            path: None,
        });
    }

    pub fn error_at_path<P, S>(&mut self, path: P, span: Span, message: S)
    where
        P: Into<PathBuf>,
        S: Into<String>,
    {
        self.diags.push(Diag {
            level: Level::Error,
            code: None,
            message: message.into(),
            span,
            path: Some(path.into()),
        });
    }

    pub fn warning<S: Into<String>>(&mut self, span: Span, message: S) {
        self.diags.push(Diag {
            level: Level::Warning,
            code: None,
            message: message.into(),
            span,
            path: None,
        });
    }

    pub fn warning_with_code<C, S>(&mut self, span: Span, code: C, message: S)
    where
        C: Into<String>,
        S: Into<String>,
    {
        self.diags.push(Diag {
            level: Level::Warning,
            code: Some(code.into()),
            message: message.into(),
            span,
            path: None,
        });
    }

    pub fn warning_at_path<P, S>(&mut self, path: P, span: Span, message: S)
    where
        P: Into<PathBuf>,
        S: Into<String>,
    {
        self.diags.push(Diag {
            level: Level::Warning,
            code: None,
            message: message.into(),
            span,
            path: Some(path.into()),
        });
    }

    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| matches!(d.level, Level::Error))
    }

    pub fn into_vec(self) -> Vec<Diag> {
        self.diags
    }

    pub fn extend(&mut self, other: Vec<Diag>) {
        self.diags.extend(other);
    }
}
