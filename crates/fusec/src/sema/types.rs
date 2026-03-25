use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct ParamSig {
    pub name: String,
    pub ty: Ty,
    pub has_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeParamSig {
    pub name: String,
    pub interface_bound: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnSig {
    pub type_params: Vec<TypeParamSig>,
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
    Html,
    Id,
    Email,
    Error,
    SelfType,
    TypeParam(String),
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
            Ty::Html => write!(f, "Html"),
            Ty::Id => write!(f, "Id"),
            Ty::Email => write!(f, "Email"),
            Ty::Error => write!(f, "Error"),
            Ty::SelfType => write!(f, "Self"),
            Ty::TypeParam(name) => write!(f, "{name}"),
            Ty::Struct(name) => write!(f, "{name}"),
            Ty::Enum(name) => write!(f, "{name}"),
            Ty::Config(name) => write!(f, "{name}"),
            Ty::External(name) => write!(f, "{name}"),
            Ty::List(inner) => write!(f, "List<{}>", inner),
            Ty::Map(key, value) => write!(f, "Map<{}, {}>", key, value),
            Ty::Option(inner) => write!(f, "{}?", inner),
            Ty::Result(ok, err) => write!(f, "{}!{}", ok, err),
            Ty::Fn(sig) => {
                write!(f, "fn")?;
                if !sig.type_params.is_empty() {
                    write!(f, "<")?;
                    for (i, param) in sig.type_params.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", param.name)?;
                    }
                    write!(f, ">")?;
                }
                write!(f, "(")?;
                for (i, param) in sig.params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", param.name, param.ty)?;
                }
                write!(f, ") -> {}", sig.ret)?;
                let mut first = true;
                for param in &sig.type_params {
                    if let Some(bound) = &param.interface_bound {
                        if first {
                            write!(f, " where ")?;
                            first = false;
                        } else {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}: {}", param.name, bound)?;
                    }
                }
                Ok(())
            }
            Ty::Task(inner) => write!(f, "Task<{}>", inner),
            Ty::Boxed(inner) => write!(f, "box {}", inner),
            Ty::Range(inner) => write!(f, "Range<{}>", inner),
            Ty::Refined { repr, .. } => write!(f, "{repr}"),
            Ty::Module(name) => write!(f, "module {name}"),
        }
    }
}
