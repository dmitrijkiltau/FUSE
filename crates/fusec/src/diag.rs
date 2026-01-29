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
        });
    }

    pub fn warning<S: Into<String>>(&mut self, span: Span, message: S) {
        self.diags.push(Diag {
            level: Level::Warning,
            message: message.into(),
            span,
        });
    }

    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| matches!(d.level, Level::Error))
    }

    pub fn into_vec(self) -> Vec<Diag> {
        self.diags
    }
}
