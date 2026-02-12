use crate::span::Span;
use serde::{Deserialize, Serialize};

pub type Doc = String;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Item {
    Import(ImportDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
    Fn(FnDecl),
    Service(ServiceDecl),
    Config(ConfigDecl),
    App(AppDecl),
    Migration(MigrationDecl),
    Test(TestDecl),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StringLit {
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportDecl {
    pub spec: ImportSpec,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ImportSpec {
    Module {
        name: Ident,
    },
    ModuleFrom {
        name: Ident,
        path: StringLit,
    },
    NamedFrom {
        names: Vec<Ident>,
        path: StringLit,
    },
    AliasFrom {
        name: Ident,
        alias: Ident,
        path: StringLit,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeDecl {
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
    pub derive: Option<TypeDerive>,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeDerive {
    pub base: Ident,
    pub without: Vec<Ident>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeRef,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<EnumVariant>,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnumVariant {
    pub name: Ident,
    pub payload: Vec<TypeRef>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FnDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Option<TypeRef>,
    pub body: Block,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeRef,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceDecl {
    pub name: Ident,
    pub base_path: StringLit,
    pub routes: Vec<RouteDecl>,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteDecl {
    pub verb: HttpVerb,
    pub path: StringLit,
    pub body_type: Option<TypeRef>,
    pub body_span: Option<Span>,
    pub ret_type: TypeRef,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpVerb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigDecl {
    pub name: Ident,
    pub fields: Vec<ConfigField>,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigField {
    pub name: Ident,
    pub ty: TypeRef,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppDecl {
    pub name: StringLit,
    pub body: Block,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationDecl {
    pub name: String,
    pub body: Block,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestDecl {
    pub name: StringLit,
    pub body: Block,
    pub doc: Option<Doc>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StmtKind {
    Let {
        name: Ident,
        ty: Option<TypeRef>,
        expr: Expr,
    },
    Var {
        name: Ident,
        ty: Option<TypeRef>,
        expr: Expr,
    },
    Assign {
        target: Expr,
        expr: Expr,
    },
    Return {
        expr: Option<Expr>,
    },
    If {
        cond: Expr,
        then_block: Block,
        else_if: Vec<(Expr, Block)>,
        else_block: Option<Block>,
    },
    Match {
        expr: Expr,
        cases: Vec<(Pattern, Block)>,
    },
    For {
        pat: Pattern,
        iter: Expr,
        block: Block,
    },
    While {
        cond: Expr,
        block: Block,
    },
    Expr(Expr),
    Break,
    Continue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ExprKind {
    Literal(Literal),
    Ident(Ident),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<CallArg>,
    },
    Member {
        base: Box<Expr>,
        name: Ident,
    },
    OptionalMember {
        base: Box<Expr>,
        name: Ident,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    OptionalIndex {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    StructLit {
        name: Ident,
        fields: Vec<StructField>,
    },
    ListLit(Vec<Expr>),
    MapLit(Vec<(Expr, Expr)>),
    InterpString(Vec<InterpPart>),
    Coalesce {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    BangChain {
        expr: Box<Expr>,
        error: Option<Box<Expr>>,
    },
    Spawn {
        block: Block,
    },
    Await {
        expr: Box<Expr>,
    },
    Box {
        expr: Box<Expr>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum InterpPart {
    Text(String),
    Expr(Expr),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallArg {
    pub name: Option<Ident>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StructField {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Null,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Range,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeRef {
    pub kind: TypeRefKind,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TypeRefKind {
    Simple(Ident),
    Generic {
        base: Ident,
        args: Vec<TypeRef>,
    },
    Optional(Box<TypeRef>),
    Result {
        ok: Box<TypeRef>,
        err: Option<Box<TypeRef>>,
    },
    Refined {
        base: Ident,
        args: Vec<Expr>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PatternKind {
    Wildcard,
    Literal(Literal),
    Ident(Ident),
    EnumVariant {
        name: Ident,
        args: Vec<Pattern>,
    },
    Struct {
        name: Ident,
        fields: Vec<PatternField>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatternField {
    pub name: Ident,
    pub pat: Box<Pattern>,
    pub span: Span,
}
