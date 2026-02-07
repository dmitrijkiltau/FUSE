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
            message: message.into(),
            span,
            path: Some(path.into()),
        });
    }

    pub fn warning<S: Into<String>>(&mut self, span: Span, message: S) {
        self.diags.push(Diag {
            level: Level::Warning,
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
