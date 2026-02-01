use std::collections::HashMap;

use crate::ast::{HttpVerb, Pattern, TypeRef};

#[derive(Clone, Debug)]
pub enum Const {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Null,
}

#[derive(Clone, Debug)]
pub enum CallKind {
    Function,
    Builtin,
}

#[derive(Clone, Debug)]
pub enum Instr {
    Push(Const),
    LoadLocal(usize),
    StoreLocal(usize),
    Pop,
    Dup,
    Neg,
    Not,
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
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfNull(usize),
    Call { name: String, argc: usize, kind: CallKind },
    Return,
    Bang { has_error: bool },
    MakeList { len: usize },
    MakeMap { len: usize },
    MakeStruct { name: String, fields: Vec<String> },
    MakeEnum { name: String, variant: String, argc: usize },
    GetField { field: String },
    InterpString { parts: usize },
    MatchLocal {
        slot: usize,
        pat: Pattern,
        bindings: Vec<(String, usize)>,
        jump: usize,
    },
    LoadConfigField { config: String, field: String },
    IterInit,
    IterNext { jump: usize },
    RuntimeError(String),
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub params: Vec<String>,
    pub ret: Option<TypeRef>,
    pub locals: usize,
    pub code: Vec<Instr>,
}

#[derive(Clone, Debug)]
pub struct ConfigField {
    pub name: String,
    pub ty: TypeRef,
    pub default_fn: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub name: String,
    pub fields: Vec<ConfigField>,
}

#[derive(Clone, Debug)]
pub struct TypeField {
    pub name: String,
    pub ty: TypeRef,
    pub default_fn: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TypeInfo {
    pub name: String,
    pub fields: Vec<TypeField>,
}

#[derive(Clone, Debug)]
pub struct EnumVariantInfo {
    pub name: String,
    pub payload: Vec<TypeRef>,
}

#[derive(Clone, Debug)]
pub struct EnumInfo {
    pub name: String,
    pub variants: Vec<EnumVariantInfo>,
}

#[derive(Clone, Debug)]
pub struct ServiceRoute {
    pub verb: HttpVerb,
    pub path: String,
    pub params: Vec<String>,
    pub body_type: Option<TypeRef>,
    pub ret_type: TypeRef,
    pub handler: String,
}

#[derive(Clone, Debug)]
pub struct Service {
    pub name: String,
    pub base_path: String,
    pub routes: Vec<ServiceRoute>,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: HashMap<String, Function>,
    pub apps: HashMap<String, Function>,
    pub configs: HashMap<String, Config>,
    pub types: HashMap<String, TypeInfo>,
    pub enums: HashMap<String, EnumInfo>,
    pub services: HashMap<String, Service>,
}

pub mod lower;
