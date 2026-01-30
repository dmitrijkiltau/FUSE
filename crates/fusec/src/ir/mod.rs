use std::collections::HashMap;

use crate::ast::{Pattern, TypeRef};

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
    MakeStruct { name: String, fields: Vec<String> },
    MakeEnum { name: String, variant: String, argc: usize },
    MatchLocal {
        slot: usize,
        pat: Pattern,
        bindings: Vec<(String, usize)>,
        jump: usize,
    },
    LoadConfigField { config: String, field: String },
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
    pub arity: usize,
}

#[derive(Clone, Debug)]
pub struct EnumInfo {
    pub name: String,
    pub variants: Vec<EnumVariantInfo>,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: HashMap<String, Function>,
    pub apps: HashMap<String, Function>,
    pub configs: HashMap<String, Config>,
    pub types: HashMap<String, TypeInfo>,
    pub enums: HashMap<String, EnumInfo>,
}

pub mod lower;
