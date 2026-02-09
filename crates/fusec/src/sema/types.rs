use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct ParamSig {
    pub name: String,
    pub ty: Ty,
    pub has_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnSig {
    pub params: Vec<ParamSig>,
    pub ret: Box<Ty>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Unknown,
    Unit,
    Int,
    Float,
    Bool,
    String,
    Bytes,
    Id,
    Email,
    Error,
    Struct(String),
    Enum(String),
    Config(String),
    External(String),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    Fn(FnSig),
    Task(Box<Ty>),
    Boxed(Box<Ty>),
    Range(Box<Ty>),
    Refined { base: Box<Ty>, repr: String },
    Module(String),
}

impl Ty {
    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, Ty::Option(_))
    }

    pub fn unwrap_optional(&self) -> Option<&Ty> {
        match self {
            Ty::Option(inner) => Some(inner),
            _ => None,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unknown => write!(f, "_"),
            Ty::Unit => write!(f, "Unit"),
            Ty::Int => write!(f, "Int"),
            Ty::Float => write!(f, "Float"),
            Ty::Bool => write!(f, "Bool"),
            Ty::String => write!(f, "String"),
            Ty::Bytes => write!(f, "Bytes"),
            Ty::Id => write!(f, "Id"),
            Ty::Email => write!(f, "Email"),
            Ty::Error => write!(f, "Error"),
            Ty::Struct(name) => write!(f, "{name}"),
            Ty::Enum(name) => write!(f, "{name}"),
            Ty::Config(name) => write!(f, "{name}"),
            Ty::External(name) => write!(f, "{name}"),
            Ty::List(inner) => write!(f, "List<{}>", inner),
            Ty::Map(key, value) => write!(f, "Map<{}, {}>", key, value),
            Ty::Option(inner) => write!(f, "{}?", inner),
            Ty::Result(ok, err) => write!(f, "{}!{}", ok, err),
            Ty::Fn(sig) => {
                write!(f, "fn(")?;
                for (i, param) in sig.params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", param.name, param.ty)?;
                }
                write!(f, ") -> {}", sig.ret)
            }
            Ty::Task(inner) => write!(f, "Task<{}>", inner),
            Ty::Boxed(inner) => write!(f, "box {}", inner),
            Ty::Range(inner) => write!(f, "Range<{}>", inner),
            Ty::Refined { repr, .. } => write!(f, "{repr}"),
            Ty::Module(name) => write!(f, "module {name}"),
        }
    }
}
