use crate::ast::{CallArg, ExprKind, Literal};

pub const HTML_ATTR_SHORTHAND_STRING_ONLY: &str =
    "html attribute shorthand only supports string literals";
pub const HTML_ATTR_SHORTHAND_MIXED_POSITIONAL: &str =
    "cannot mix html attribute shorthand with positional arguments";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalizationPhase {
    TypeCheck,
    Lowering,
    Execution,
}

pub fn validate_named_args_for_phase(
    args: &[CallArg],
    phase: CanonicalizationPhase,
) -> Option<&'static str> {
    if !args.iter().any(|arg| arg.name.is_some()) {
        return None;
    }
    let mut child_seen = false;
    for arg in args {
        if arg.name.is_some() {
            if !matches!(&arg.value.kind, ExprKind::Literal(Literal::String(_))) {
                return Some(HTML_ATTR_SHORTHAND_STRING_ONLY);
            }
            continue;
        }
        if arg.is_block_sugar && !child_seen {
            child_seen = true;
            continue;
        }
        return Some(HTML_ATTR_SHORTHAND_MIXED_POSITIONAL);
    }
    Some(match phase {
        CanonicalizationPhase::TypeCheck => {
            "html attribute shorthand must be canonicalized before type checking"
        }
        CanonicalizationPhase::Lowering => {
            "html attribute shorthand must be canonicalized before lowering"
        }
        CanonicalizationPhase::Execution => {
            "html attribute shorthand must be canonicalized before execution"
        }
    })
}
