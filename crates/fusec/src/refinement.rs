use crate::ast::{BinaryOp, Expr, ExprKind, Literal, UnaryOp};
use crate::span::Span;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberLiteral {
    Int(i64),
    Float(f64),
}

impl NumberLiteral {
    pub fn as_i64(self) -> Option<i64> {
        match self {
            NumberLiteral::Int(v) => Some(v),
            NumberLiteral::Float(v) if v.fract() == 0.0 => Some(v as i64),
            NumberLiteral::Float(_) => None,
        }
    }

    pub fn as_f64(self) -> f64 {
        match self {
            NumberLiteral::Int(v) => v as f64,
            NumberLiteral::Float(v) => v,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RefinementConstraint {
    Range {
        min: NumberLiteral,
        max: NumberLiteral,
        span: Span,
    },
    Regex {
        pattern: String,
        span: Span,
    },
    Predicate {
        name: String,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RefinementParseError {
    pub span: Span,
    pub message: String,
}

pub fn parse_constraints(args: &[Expr]) -> Result<Vec<RefinementConstraint>, RefinementParseError> {
    if args.is_empty() {
        return Err(RefinementParseError {
            span: Span::default(),
            message: "refined type expects at least one constraint".to_string(),
        });
    }

    // Backward-compatible shorthand: T(min, max)
    if args.len() == 2 {
        if let (Some(min), Some(max)) = (literal_number(&args[0]), literal_number(&args[1])) {
            return Ok(vec![RefinementConstraint::Range {
                min,
                max,
                span: args[0].span.merge(args[1].span),
            }]);
        }
    }

    let mut out = Vec::with_capacity(args.len());
    for expr in args {
        out.push(parse_constraint(expr)?);
    }
    Ok(out)
}

fn parse_constraint(expr: &Expr) -> Result<RefinementConstraint, RefinementParseError> {
    if let ExprKind::Binary {
        op: BinaryOp::Range,
        left,
        right,
    } = &expr.kind
    {
        let min = literal_number(left).ok_or_else(|| RefinementParseError {
            span: left.span,
            message: "range lower bound must be a numeric literal".to_string(),
        })?;
        let max = literal_number(right).ok_or_else(|| RefinementParseError {
            span: right.span,
            message: "range upper bound must be a numeric literal".to_string(),
        })?;
        return Ok(RefinementConstraint::Range {
            min,
            max,
            span: expr.span,
        });
    }

    let ExprKind::Call { callee, args } = &expr.kind else {
        return Err(RefinementParseError {
            span: expr.span,
            message: "unsupported refinement constraint; expected 1..10, regex(\"...\"), or predicate(fn_name)".to_string(),
        });
    };
    let ExprKind::Ident(ident) = &callee.kind else {
        return Err(RefinementParseError {
            span: callee.span,
            message: "refinement constraint call must be regex(...) or predicate(...)".to_string(),
        });
    };
    if args.len() != 1 {
        return Err(RefinementParseError {
            span: expr.span,
            message: format!("{}() expects exactly one positional argument", ident.name),
        });
    }
    let arg = &args[0];
    if arg.name.is_some() || arg.is_block_sugar {
        return Err(RefinementParseError {
            span: arg.span,
            message: format!("{}() expects a positional argument", ident.name),
        });
    }

    match ident.name.as_str() {
        "regex" => match &arg.value.kind {
            ExprKind::Literal(Literal::String(pattern)) => Ok(RefinementConstraint::Regex {
                pattern: pattern.clone(),
                span: expr.span,
            }),
            _ => Err(RefinementParseError {
                span: arg.value.span,
                message: "regex() expects a string literal pattern".to_string(),
            }),
        },
        "predicate" => match &arg.value.kind {
            ExprKind::Ident(name) => Ok(RefinementConstraint::Predicate {
                name: name.name.clone(),
                span: arg.value.span,
            }),
            _ => Err(RefinementParseError {
                span: arg.value.span,
                message: "predicate() expects a function identifier".to_string(),
            }),
        },
        _ => Err(RefinementParseError {
            span: ident.span,
            message: format!(
                "unknown refinement constraint {}; expected regex(...) or predicate(...)",
                ident.name
            ),
        }),
    }
}

pub fn literal_i64(expr: &Expr) -> Option<i64> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(v)) => Some(*v),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match &expr.kind {
            ExprKind::Literal(Literal::Int(v)) => Some(-v),
            _ => None,
        },
        _ => None,
    }
}

pub fn literal_f64(expr: &Expr) -> Option<f64> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(v)) => Some(*v as f64),
        ExprKind::Literal(Literal::Float(v)) => Some(*v),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match &expr.kind {
            ExprKind::Literal(Literal::Int(v)) => Some(-(*v as f64)),
            ExprKind::Literal(Literal::Float(v)) => Some(-*v),
            _ => None,
        },
        _ => None,
    }
}

pub fn literal_number(expr: &Expr) -> Option<NumberLiteral> {
    if let Some(v) = literal_i64(expr) {
        return Some(NumberLiteral::Int(v));
    }
    literal_f64(expr).map(NumberLiteral::Float)
}

pub fn base_is_string_like(base: &str) -> bool {
    matches!(base, "String" | "Id" | "Email")
}
