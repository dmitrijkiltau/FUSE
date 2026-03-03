use crate::ast::CallArg;

pub const HTML_ATTR_MAP_DIAG_CODE: &str = "FUSE_HTML_ATTR_MAP";
pub const HTML_ATTR_COMMA_DIAG_CODE: &str = "FUSE_HTML_ATTR_COMMA";
pub const HTML_ATTR_MAP_MESSAGE: &str = "map literal is not valid for HTML tag attributes";
pub const HTML_ATTR_COMMA_MESSAGE: &str = "commas are not allowed between HTML tag attributes";
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
