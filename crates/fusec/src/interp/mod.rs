use std::collections::HashMap;

use fuse_rt::{config as rt_config, error as rt_error, json as rt_json, validate as rt_validate};

use crate::ast::{
    AppDecl, BinaryOp, Block, ConfigDecl, EnumDecl, Expr, ExprKind, FnDecl, Ident, Item, Literal,
    Pattern, PatternField, PatternKind, Program, Stmt, StmtKind, StructField, TypeDecl, TypeRef,
    TypeRefKind, UnaryOp,
};

#[derive(Clone, Debug)]
pub enum Value {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Null,
    Struct {
        name: String,
        fields: HashMap<String, Value>,
    },
    Enum {
        name: String,
        variant: String,
        payload: Vec<Value>,
    },
    EnumCtor {
        name: String,
        variant: String,
    },
    ResultOk(Box<Value>),
    ResultErr(Box<Value>),
    Config(String),
    Function(String),
    Builtin(String),
}

impl Value {
    pub fn to_string_value(&self) -> String {
        match self {
            Value::Unit => "()".to_string(),
            Value::Int(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Bool(v) => v.to_string(),
            Value::String(v) => v.clone(),
            Value::Null => "null".to_string(),
            Value::Struct { name, fields } => match fields.get("message") {
                Some(Value::String(msg)) => format!("{name}({msg})"),
                _ => format!("<{name}>"),
            },
            Value::Enum {
                name,
                variant,
                payload,
            } => {
                if payload.is_empty() {
                    format!("{name}.{variant}")
                } else {
                    let args = payload
                        .iter()
                        .map(|val| val.to_string_value())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{name}.{variant}({args})")
                }
            }
            Value::EnumCtor { name, variant } => format!("<enum {name}.{variant}>"),
            Value::ResultOk(val) => format!("Ok({})", val.to_string_value()),
            Value::ResultErr(val) => format!("Err({})", val.to_string_value()),
            Value::Config(name) => format!("<config {name}>"),
            Value::Function(name) => format!("<fn {name}>"),
            Value::Builtin(name) => format!("<builtin {name}>"),
        }
    }
}

pub fn format_error_value(value: &Value) -> String {
    if let Value::Struct { name, fields } = value {
        if name == "ValidationError" {
            let message = match fields.get("message") {
                Some(Value::String(msg)) => msg.as_str(),
                _ => "validation failed",
            };
            let json = rt_error::validation_error_json(message, &[]);
            return rt_json::encode(&json);
        }
        if name == "Error" {
            let message = match fields.get("message") {
                Some(Value::String(msg)) => msg.as_str(),
                _ => "error",
            };
            let json = rt_error::error_json("error", message, None);
            return rt_json::encode(&json);
        }
    }
    value.to_string_value()
}

#[derive(Debug)]
enum ExecError {
    Runtime(String),
    Return(Value),
    Error(Value),
}

type ExecResult<T> = Result<T, ExecError>;

#[derive(Default)]
struct Env {
    scopes: Vec<HashMap<String, Value>>,
}

impl Env {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn insert(&mut self, name: &str, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), value);
        }
    }

    fn assign(&mut self, name: &str, value: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return true;
            }
        }
        false
    }

    fn get(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(val) = scope.get(name) {
                return Some(val.clone());
            }
        }
        None
    }
}

pub struct Interpreter<'a> {
    program: &'a Program,
    env: Env,
    configs: HashMap<String, HashMap<String, Value>>,
    functions: HashMap<String, &'a FnDecl>,
    apps: Vec<&'a AppDecl>,
    types: HashMap<String, &'a TypeDecl>,
    config_decls: HashMap<String, &'a ConfigDecl>,
    enums: HashMap<String, &'a EnumDecl>,
}

impl<'a> Interpreter<'a> {
    pub fn new(program: &'a Program) -> Self {
        let mut functions = HashMap::new();
        let mut apps = Vec::new();
        let mut types = HashMap::new();
        let mut config_decls = HashMap::new();
        let mut enums = HashMap::new();
        for item in &program.items {
            match item {
                Item::Fn(decl) => {
                    functions.insert(decl.name.name.clone(), decl);
                }
                Item::App(app) => {
                    apps.push(app);
                }
                Item::Type(decl) => {
                    types.insert(decl.name.name.clone(), decl);
                }
                Item::Config(decl) => {
                    config_decls.insert(decl.name.name.clone(), decl);
                }
                Item::Enum(decl) => {
                    enums.insert(decl.name.name.clone(), decl);
                }
                _ => {}
            }
        }
        Self {
            program,
            env: Env::new(),
            configs: HashMap::new(),
            functions,
            apps,
            types,
            config_decls,
            enums,
        }
    }

    pub fn run_app(&mut self, name: Option<&str>) -> Result<(), String> {
        if let Err(err) = self.eval_configs() {
            return Err(self.render_exec_error(err));
        }
        let app = if let Some(name) = name {
            self.apps
                .iter()
                .find(|app| app.name.value == name)
                .copied()
                .ok_or_else(|| format!("app not found: {name}"))?
        } else {
            self.apps
                .first()
                .copied()
                .ok_or_else(|| "no app found".to_string())?
        };
        match self.eval_block(&app.body) {
            Ok(_) => Ok(()),
            Err(ExecError::Return(_)) => Ok(()),
            Err(err) => Err(self.render_exec_error(err)),
        }
    }

    fn render_exec_error(&self, err: ExecError) -> String {
        match err {
            ExecError::Runtime(msg) => msg,
            ExecError::Error(value) => format_error_value(&value),
            ExecError::Return(value) => format!("unexpected return: {}", value.to_string_value()),
        }
    }

    fn eval_configs(&mut self) -> ExecResult<()> {
        for item in &self.program.items {
            if let Item::Config(decl) = item {
                let name = decl.name.name.clone();
                self.configs.insert(name.clone(), HashMap::new());
                for field in &decl.fields {
                    let key = self.config_env_key(&decl.name.name, &field.name.name);
                    let path = format!("{}.{}", decl.name.name, field.name.name);
                    let value = match std::env::var(&key) {
                        Ok(raw) => {
                            let value = self.parse_env_value(&field.ty, &raw)?;
                            self.validate_value(&value, &field.ty, &path)?;
                            value
                        }
                        Err(_) => {
                            let value = self.eval_expr(&field.value)?;
                            self.validate_value(&value, &field.ty, &path)?;
                            value
                        }
                    };
                    if let Some(map) = self.configs.get_mut(&name) {
                        map.insert(field.name.name.clone(), value);
                    }
                }
            }
        }
        Ok(())
    }

    fn eval_block(&mut self, block: &Block) -> ExecResult<Value> {
        self.env.push();
        let mut last = Value::Unit;
        for stmt in &block.stmts {
            match self.eval_stmt(stmt) {
                Ok(value) => last = value,
                Err(ExecError::Return(value)) => {
                    self.env.pop();
                    return Err(ExecError::Return(value));
                }
                Err(err) => {
                    self.env.pop();
                    return Err(err);
                }
            }
        }
        self.env.pop();
        Ok(last)
    }

    fn eval_stmt(&mut self, stmt: &Stmt) -> ExecResult<Value> {
        match &stmt.kind {
            StmtKind::Let { name, expr, .. } => {
                let value = self.eval_expr(expr)?;
                self.env.insert(&name.name, value);
                Ok(Value::Unit)
            }
            StmtKind::Var { name, expr, .. } => {
                let value = self.eval_expr(expr)?;
                self.env.insert(&name.name, value);
                Ok(Value::Unit)
            }
            StmtKind::Assign { target, expr } => match &target.kind {
                ExprKind::Ident(ident) => {
                    let value = self.eval_expr(expr)?;
                    if !self.env.assign(&ident.name, value) {
                        return Err(ExecError::Runtime(format!(
                            "unknown variable {}",
                            ident.name
                        )));
                    }
                    Ok(Value::Unit)
                }
                _ => Err(ExecError::Runtime(
                    "unsupported assignment target".to_string(),
                )),
            },
            StmtKind::Return { expr } => {
                let value = match expr {
                    Some(expr) => self.eval_expr(expr)?,
                    None => Value::Unit,
                };
                Err(ExecError::Return(value))
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                let cond_val = self.eval_expr(cond)?;
                if self.as_bool(&cond_val)? {
                    self.eval_block(then_block)
                } else {
                    for (cond, block) in else_if {
                        let cond_val = self.eval_expr(cond)?;
                        if self.as_bool(&cond_val)? {
                            return self.eval_block(block);
                        }
                    }
                    if let Some(block) = else_block {
                        self.eval_block(block)
                    } else {
                        Ok(Value::Unit)
                    }
                }
            }
            StmtKind::Expr(expr) => {
                let value = self.eval_expr(expr)?;
                Ok(value)
            }
            StmtKind::Match { expr, cases } => {
                let value = self.eval_expr(expr)?;
                for (pat, block) in cases {
                    let mut bindings = HashMap::new();
                    if self.match_pattern(&value, pat, &mut bindings)? {
                        self.env.push();
                        for (name, value) in bindings {
                            self.env.insert(&name, value);
                        }
                        let result = self.eval_block(block);
                        self.env.pop();
                        return result;
                    }
                }
                Ok(Value::Unit)
            }
            | StmtKind::For { .. }
            | StmtKind::While { .. }
            | StmtKind::Break
            | StmtKind::Continue => Err(ExecError::Runtime(
                "statement not supported in interpreter yet".to_string(),
            )),
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> ExecResult<Value> {
        match &expr.kind {
            ExprKind::Literal(lit) => Ok(self.value_from_literal(lit)),
            ExprKind::Ident(ident) => self.resolve_ident(&ident.name),
            ExprKind::Binary { op, left, right } => {
                let left_val = self.eval_expr(left)?;
                let right_val = self.eval_expr(right)?;
                match op {
                    BinaryOp::Add => self.eval_add(left_val, right_val),
                    BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                        self.eval_arith(op, left_val, right_val)
                    }
                    BinaryOp::Eq
                    | BinaryOp::NotEq
                    | BinaryOp::Lt
                    | BinaryOp::LtEq
                    | BinaryOp::Gt
                    | BinaryOp::GtEq => self.eval_compare(op, left_val, right_val),
                    BinaryOp::And | BinaryOp::Or => self.eval_bool(op, left_val, right_val),
                    BinaryOp::Range => Err(ExecError::Runtime(
                        "range not supported in interpreter yet".to_string(),
                    )),
                }
            }
            ExprKind::Unary { op, expr } => {
                let value = self.eval_expr(expr)?;
                match op {
                    UnaryOp::Neg => match value {
                        Value::Int(v) => Ok(Value::Int(-v)),
                        Value::Float(v) => Ok(Value::Float(-v)),
                        _ => Err(ExecError::Runtime("unary '-' expects number".to_string())),
                    },
                    UnaryOp::Not => Ok(Value::Bool(!self.as_bool(&value)?)),
                }
            }
            ExprKind::Call { callee, args } => {
                let callee_val = self.eval_expr(callee)?;
                let mut arg_vals = Vec::new();
                for arg in args {
                    arg_vals.push(self.eval_expr(&arg.value)?);
                }
                self.eval_call(callee_val, arg_vals)
            }
            ExprKind::Member { base, name } => {
                if let ExprKind::Ident(ident) = &base.kind {
                    if self.enums.contains_key(&ident.name) {
                        return self.eval_enum_member(&ident.name, &name.name);
                    }
                }
                let base_val = self.eval_expr(base)?;
                self.eval_member(base_val, &name.name)
            }
            ExprKind::OptionalMember { base, name } => {
                let base_val = self.eval_expr(base)?;
                if matches!(base_val, Value::Null) {
                    Ok(Value::Null)
                } else {
                    self.eval_member(base_val, &name.name)
                }
            }
            ExprKind::StructLit { name, fields } => self.eval_struct_lit(name, fields),
            ExprKind::ListLit(_) => Err(ExecError::Runtime(
                "list literals not supported in interpreter yet".to_string(),
            )),
            ExprKind::MapLit(_) => Err(ExecError::Runtime(
                "map literals not supported in interpreter yet".to_string(),
            )),
            ExprKind::InterpString(_) => Err(ExecError::Runtime(
                "interp strings not supported in interpreter yet".to_string(),
            )),
            ExprKind::Coalesce { left, right } => {
                let left_val = self.eval_expr(left)?;
                if matches!(left_val, Value::Null) {
                    self.eval_expr(right)
                } else {
                    Ok(left_val)
                }
            }
            ExprKind::BangChain { expr, error } => self.eval_bang_chain(expr, error.as_deref()),
            ExprKind::Spawn { .. } => Err(ExecError::Runtime(
                "spawn not supported in interpreter yet".to_string(),
            )),
            ExprKind::Await { .. } => Err(ExecError::Runtime(
                "await not supported in interpreter yet".to_string(),
            )),
            ExprKind::Box { .. } => Err(ExecError::Runtime(
                "box not supported in interpreter yet".to_string(),
            )),
        }
    }

    fn resolve_ident(&self, name: &str) -> ExecResult<Value> {
        if let Some(val) = self.env.get(name) {
            return Ok(val);
        }
        if self.functions.contains_key(name) {
            return Ok(Value::Function(name.to_string()));
        }
        if self.config_decls.contains_key(name) {
            return Ok(Value::Config(name.to_string()));
        }
        match name {
            "print" | "env" | "serve" => Ok(Value::Builtin(name.to_string())),
            _ => Err(ExecError::Runtime(format!("unknown identifier {name}"))),
        }
    }

    fn eval_call(&mut self, callee: Value, args: Vec<Value>) -> ExecResult<Value> {
        match callee {
            Value::Builtin(name) => self.eval_builtin(&name, args),
            Value::Function(name) => self.eval_function(&name, args),
            Value::EnumCtor { name, variant } => {
                let arity = self
                    .enum_variant_arity(&name, &variant)
                    .ok_or_else(|| {
                        ExecError::Runtime(format!("unknown variant {name}.{variant}"))
                    })?;
                if arity != args.len() {
                    return Err(ExecError::Runtime(format!(
                        "variant {name}.{variant} expects {} value(s), got {}",
                        arity,
                        args.len()
                    )));
                }
                Ok(Value::Enum {
                    name,
                    variant,
                    payload: args,
                })
            }
            _ => Err(ExecError::Runtime("call target is not callable".to_string())),
        }
    }

    fn eval_builtin(&mut self, name: &str, args: Vec<Value>) -> ExecResult<Value> {
        match name {
            "print" => {
                let text = args.get(0).map(|v| v.to_string_value()).unwrap_or_default();
                println!("{text}");
                Ok(Value::Unit)
            }
            "env" => {
                let key = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "env expects a string argument".to_string(),
                        ))
                    }
                };
                match std::env::var(key) {
                    Ok(value) => Ok(Value::String(value)),
                    Err(_) => Ok(Value::Null),
                }
            }
            "serve" => Ok(Value::Unit),
            _ => Err(ExecError::Runtime(format!("unknown builtin {name}"))),
        }
    }

    fn eval_function(&mut self, name: &str, args: Vec<Value>) -> ExecResult<Value> {
        let decl = match self.functions.get(name) {
            Some(decl) => *decl,
            None => return Err(ExecError::Runtime(format!("unknown function {name}"))),
        };
        self.env.push();
        for (idx, param) in decl.params.iter().enumerate() {
            let value = if idx < args.len() {
                args[idx].clone()
            } else if let Some(default) = &param.default {
                self.eval_expr(default)?
            } else {
                return Err(ExecError::Runtime(format!(
                    "missing argument {}",
                    param.name.name
                )));
            };
            self.env.insert(&param.name.name, value);
        }
        let result = self.eval_block(&decl.body);
        self.env.pop();
        match result {
            Ok(value) => self.wrap_function_result(&decl.ret, value),
            Err(ExecError::Return(value)) => self.wrap_function_result(&decl.ret, value),
            Err(ExecError::Error(value)) => {
                if self.is_result_type(decl.ret.as_ref()) {
                    Ok(Value::ResultErr(Box::new(value)))
                } else {
                    Err(ExecError::Error(value))
                }
            }
            Err(ExecError::Runtime(msg)) => Err(ExecError::Runtime(msg)),
        }
    }

    fn wrap_function_result(&self, ret: &Option<TypeRef>, value: Value) -> ExecResult<Value> {
        if self.is_result_type(ret.as_ref()) {
            match value {
                Value::ResultOk(_) | Value::ResultErr(_) => Ok(value),
                _ => Ok(Value::ResultOk(Box::new(value))),
            }
        } else {
            Ok(value)
        }
    }

    fn is_result_type(&self, ty: Option<&TypeRef>) -> bool {
        match ty {
            Some(ty) => match &ty.kind {
                TypeRefKind::Result { .. } => true,
                TypeRefKind::Generic { base, .. } => base.name == "Result",
                _ => false,
            },
            None => false,
        }
    }

    fn eval_member(&mut self, base: Value, field: &str) -> ExecResult<Value> {
        match base {
            Value::Config(name) => {
                let map = self
                    .configs
                    .get(&name)
                    .ok_or_else(|| ExecError::Runtime(format!("unknown config {name}")))?;
                map.get(field)
                    .cloned()
                    .ok_or_else(|| ExecError::Runtime(format!("unknown config field {name}.{field}")))
            }
            Value::Struct { fields, .. } => fields
                .get(field)
                .cloned()
                .ok_or_else(|| ExecError::Runtime(format!("unknown field {field}"))),
            _ => Err(ExecError::Runtime(
                "member access not supported on this value".to_string(),
            )),
        }
    }

    fn eval_enum_member(&self, enum_name: &str, variant: &str) -> ExecResult<Value> {
        let arity = self
            .enum_variant_arity(enum_name, variant)
            .ok_or_else(|| ExecError::Runtime(format!("unknown variant {enum_name}.{variant}")))?;
        if arity == 0 {
            Ok(Value::Enum {
                name: enum_name.to_string(),
                variant: variant.to_string(),
                payload: Vec::new(),
            })
        } else {
            Ok(Value::EnumCtor {
                name: enum_name.to_string(),
                variant: variant.to_string(),
            })
        }
    }

    fn enum_variant_arity(&self, enum_name: &str, variant: &str) -> Option<usize> {
        self.enums.get(enum_name).and_then(|decl| {
            decl.variants
                .iter()
                .find(|v| v.name.name == variant)
                .map(|v| v.payload.len())
        })
    }

    fn enum_variant_exists(&self, enum_name: &str, variant: &str) -> bool {
        self.enum_variant_arity(enum_name, variant).is_some()
    }

    fn eval_struct_lit(&mut self, name: &Ident, fields: &[StructField]) -> ExecResult<Value> {
        let decl = match self.types.get(&name.name) {
            Some(decl) => *decl,
            None => {
                return Err(ExecError::Runtime(format!(
                    "unknown type {}",
                    name.name
                )))
            }
        };
        let mut values = HashMap::new();
        for field in fields {
            if values.contains_key(&field.name.name) {
                return Err(ExecError::Runtime(format!(
                    "duplicate field {}",
                    field.name.name
                )));
            }
            let field_decl = decl
                .fields
                .iter()
                .find(|f| f.name.name == field.name.name)
                .ok_or_else(|| {
                    ExecError::Runtime(format!("unknown field {}.{}", name.name, field.name.name))
                })?;
            let value = self.eval_expr(&field.value)?;
            let path = format!("{}.{}", name.name, field.name.name);
            self.validate_value(&value, &field_decl.ty, &path)?;
            values.insert(field.name.name.clone(), value);
        }
        for field_decl in &decl.fields {
            if values.contains_key(&field_decl.name.name) {
                continue;
            }
            let path = format!("{}.{}", name.name, field_decl.name.name);
            if let Some(default) = &field_decl.default {
                let value = self.eval_expr(default)?;
                self.validate_value(&value, &field_decl.ty, &path)?;
                values.insert(field_decl.name.name.clone(), value);
            } else if self.is_optional_type(&field_decl.ty) {
                values.insert(field_decl.name.name.clone(), Value::Null);
            } else {
                return Err(ExecError::Runtime(format!(
                    "missing field {}.{}",
                    name.name, field_decl.name.name
                )));
            }
        }
        Ok(Value::Struct {
            name: name.name.clone(),
            fields: values,
        })
    }

    fn eval_bang_chain(&mut self, expr: &Expr, error: Option<&Expr>) -> ExecResult<Value> {
        let value = self.eval_expr(expr)?;
        match value {
            Value::Null => {
                let err_value = match error {
                    Some(expr) => self.eval_expr(expr)?,
                    None => self.default_error_value("missing value"),
                };
                Err(ExecError::Error(err_value))
            }
            Value::ResultOk(ok) => Ok(*ok),
            Value::ResultErr(err) => {
                let err_value = match error {
                    Some(expr) => self.eval_expr(expr)?,
                    None => *err,
                };
                Err(ExecError::Error(err_value))
            }
            _ => Err(ExecError::Runtime(
                "?! expects Option or Result".to_string(),
            )),
        }
    }

    fn value_from_literal(&self, lit: &Literal) -> Value {
        match lit {
            Literal::Int(v) => Value::Int(*v),
            Literal::Float(v) => Value::Float(*v),
            Literal::Bool(v) => Value::Bool(*v),
            Literal::String(v) => Value::String(v.clone()),
            Literal::Null => Value::Null,
        }
    }

    fn as_bool(&self, value: &Value) -> ExecResult<bool> {
        match value {
            Value::Bool(v) => Ok(*v),
            _ => Err(ExecError::Runtime("condition must be a Bool".to_string())),
        }
    }

    fn eval_add(&self, left: Value, right: Value) -> ExecResult<Value> {
        match (left, right) {
            (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{a}{b}"))),
            (Value::String(a), b) => Ok(Value::String(format!("{a}{}", b.to_string_value()))),
            (a, Value::String(b)) => Ok(Value::String(format!("{}{}", a.to_string_value(), b))),
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
            _ => Err(ExecError::Runtime("unsupported + operands".to_string())),
        }
    }

    fn eval_arith(&self, op: &BinaryOp, left: Value, right: Value) -> ExecResult<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => match op {
                BinaryOp::Sub => Ok(Value::Int(a - b)),
                BinaryOp::Mul => Ok(Value::Int(a * b)),
                BinaryOp::Div => Ok(Value::Int(a / b)),
                BinaryOp::Mod => Ok(Value::Int(a % b)),
                _ => Err(ExecError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Float(a), Value::Float(b)) => match op {
                BinaryOp::Sub => Ok(Value::Float(a - b)),
                BinaryOp::Mul => Ok(Value::Float(a * b)),
                BinaryOp::Div => Ok(Value::Float(a / b)),
                BinaryOp::Mod => Err(ExecError::Runtime(
                    "mod not supported for float".to_string(),
                )),
                _ => Err(ExecError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Int(a), Value::Float(b)) => match op {
                BinaryOp::Sub => Ok(Value::Float(a as f64 - b)),
                BinaryOp::Mul => Ok(Value::Float(a as f64 * b)),
                BinaryOp::Div => Ok(Value::Float(a as f64 / b)),
                BinaryOp::Mod => Err(ExecError::Runtime(
                    "mod not supported for float".to_string(),
                )),
                _ => Err(ExecError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Float(a), Value::Int(b)) => match op {
                BinaryOp::Sub => Ok(Value::Float(a - b as f64)),
                BinaryOp::Mul => Ok(Value::Float(a * b as f64)),
                BinaryOp::Div => Ok(Value::Float(a / b as f64)),
                BinaryOp::Mod => Err(ExecError::Runtime(
                    "mod not supported for float".to_string(),
                )),
                _ => Err(ExecError::Runtime("unsupported arithmetic op".to_string())),
            },
            _ => Err(ExecError::Runtime(
                "unsupported arithmetic operands".to_string(),
            )),
        }
    }

    fn eval_compare(&self, op: &BinaryOp, left: Value, right: Value) -> ExecResult<Value> {
        let result = match (left, right) {
            (Value::Int(a), Value::Int(b)) => match op {
                BinaryOp::Eq => a == b,
                BinaryOp::NotEq => a != b,
                BinaryOp::Lt => a < b,
                BinaryOp::LtEq => a <= b,
                BinaryOp::Gt => a > b,
                BinaryOp::GtEq => a >= b,
                _ => return Err(ExecError::Runtime("unsupported comparison".to_string())),
            },
            (Value::Float(a), Value::Float(b)) => match op {
                BinaryOp::Eq => a == b,
                BinaryOp::NotEq => a != b,
                BinaryOp::Lt => a < b,
                BinaryOp::LtEq => a <= b,
                BinaryOp::Gt => a > b,
                BinaryOp::GtEq => a >= b,
                _ => return Err(ExecError::Runtime("unsupported comparison".to_string())),
            },
            (Value::String(a), Value::String(b)) => match op {
                BinaryOp::Eq => a == b,
                BinaryOp::NotEq => a != b,
                _ => return Err(ExecError::Runtime("unsupported string comparison".to_string())),
            },
            _ => return Err(ExecError::Runtime("unsupported comparison operands".to_string())),
        };
        Ok(Value::Bool(result))
    }

    fn eval_bool(&self, op: &BinaryOp, left: Value, right: Value) -> ExecResult<Value> {
        let left = self.as_bool(&left)?;
        let right = self.as_bool(&right)?;
        let result = match op {
            BinaryOp::And => left && right,
            BinaryOp::Or => left || right,
            _ => return Err(ExecError::Runtime("unsupported boolean op".to_string())),
        };
        Ok(Value::Bool(result))
    }

    fn match_pattern(
        &self,
        value: &Value,
        pat: &Pattern,
        bindings: &mut HashMap<String, Value>,
    ) -> ExecResult<bool> {
        match &pat.kind {
            PatternKind::Wildcard => Ok(true),
            PatternKind::Literal(lit) => Ok(self.literal_matches(value, lit)),
            PatternKind::Ident(ident) => self.match_ident_pattern(value, ident, bindings),
            PatternKind::EnumVariant { name, args } => {
                self.match_enum_variant_pattern(value, &name.name, args, bindings)
            }
            PatternKind::Struct { name, fields } => {
                self.match_struct_pattern(value, &name.name, fields, bindings)
            }
        }
    }

    fn match_ident_pattern(
        &self,
        value: &Value,
        ident: &Ident,
        bindings: &mut HashMap<String, Value>,
    ) -> ExecResult<bool> {
        if let Some(is_match) = self.match_builtin_variant(value, &ident.name)? {
            return Ok(is_match);
        }
        if let Value::Enum { variant, payload, .. } = value {
            if variant == &ident.name {
                return Ok(payload.is_empty());
            }
            if let Value::Enum { name, .. } = value {
                if self.enum_variant_exists(name, &ident.name) {
                    return Ok(false);
                }
            }
        }
        bindings.insert(ident.name.clone(), value.clone());
        Ok(true)
    }

    fn match_builtin_variant(&self, value: &Value, name: &str) -> ExecResult<Option<bool>> {
        match name {
            "None" => Ok(Some(matches!(value, Value::Null))),
            "Some" | "Ok" | "Err" => Ok(Some(false)),
            _ => Ok(None),
        }
    }

    fn match_enum_variant_pattern(
        &self,
        value: &Value,
        name: &str,
        args: &[Pattern],
        bindings: &mut HashMap<String, Value>,
    ) -> ExecResult<bool> {
        match name {
            "Some" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime("Some expects 1 pattern".to_string()));
                }
                if matches!(value, Value::Null) {
                    return Ok(false);
                }
                self.match_pattern(value, &args[0], bindings)
            }
            "None" => {
                if !args.is_empty() {
                    return Err(ExecError::Runtime("None expects no patterns".to_string()));
                }
                Ok(matches!(value, Value::Null))
            }
            "Ok" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime("Ok expects 1 pattern".to_string()));
                }
                match value {
                    Value::ResultOk(inner) => self.match_pattern(inner, &args[0], bindings),
                    _ => Ok(false),
                }
            }
            "Err" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime("Err expects 1 pattern".to_string()));
                }
                match value {
                    Value::ResultErr(inner) => self.match_pattern(inner, &args[0], bindings),
                    _ => Ok(false),
                }
            }
            _ => match value {
                Value::Enum {
                    variant, payload, ..
                } => {
                    if variant != name {
                        return Ok(false);
                    }
                    if payload.len() != args.len() {
                        return Ok(false);
                    }
                    for (arg, val) in args.iter().zip(payload.iter()) {
                        if !self.match_pattern(val, arg, bindings)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                _ => Ok(false),
            },
        }
    }

    fn match_struct_pattern(
        &self,
        value: &Value,
        name: &str,
        fields: &[PatternField],
        bindings: &mut HashMap<String, Value>,
    ) -> ExecResult<bool> {
        let (struct_name, struct_fields) = match value {
            Value::Struct { name, fields } => (name, fields),
            _ => return Ok(false),
        };
        if struct_name != name {
            return Ok(false);
        }
        for field in fields {
            let value = match struct_fields.get(&field.name.name) {
                Some(value) => value,
                None => return Ok(false),
            };
            if !self.match_pattern(value, &field.pat, bindings)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn literal_matches(&self, value: &Value, lit: &Literal) -> bool {
        match (value, lit) {
            (Value::Int(a), Literal::Int(b)) => a == b,
            (Value::Float(a), Literal::Float(b)) => a == b,
            (Value::Bool(a), Literal::Bool(b)) => a == b,
            (Value::String(a), Literal::String(b)) => a == b,
            (Value::Null, Literal::Null) => true,
            _ => false,
        }
    }

    fn config_env_key(&self, config: &str, field: &str) -> String {
        rt_config::env_key(config, field)
    }

    fn parse_env_value(&self, ty: &TypeRef, raw: &str) -> ExecResult<Value> {
        let raw = raw.trim();
        match &ty.kind {
            TypeRefKind::Optional(inner) => {
                if raw.eq_ignore_ascii_case("null") || raw.is_empty() {
                    Ok(Value::Null)
                } else {
                    self.parse_env_value(inner, raw)
                }
            }
            TypeRefKind::Refined { base, .. } => self.parse_simple_env(&base.name, raw),
            TypeRefKind::Simple(ident) => self.parse_simple_env(&ident.name, raw),
            TypeRefKind::Result { .. } => Err(ExecError::Runtime(
                "Result is not supported for config env overrides".to_string(),
            )),
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(ExecError::Runtime(
                            "Option expects 1 type argument".to_string(),
                        ));
                    }
                    if raw.eq_ignore_ascii_case("null") || raw.is_empty() {
                        Ok(Value::Null)
                    } else {
                        self.parse_env_value(&args[0], raw)
                    }
                }
                "Result" => Err(ExecError::Runtime(
                    "Result is not supported for config env overrides".to_string(),
                )),
                _ => Err(ExecError::Runtime(
                    "config env overrides only support simple types".to_string(),
                )),
            },
        }
    }

    fn parse_simple_env(&self, name: &str, raw: &str) -> ExecResult<Value> {
        match name {
            "Int" => raw
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| ExecError::Runtime(format!("invalid Int: {raw}"))),
            "Float" => raw
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| ExecError::Runtime(format!("invalid Float: {raw}"))),
            "Bool" => match raw.to_ascii_lowercase().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(ExecError::Runtime(format!("invalid Bool: {raw}"))),
            },
            "String" | "Id" | "Email" | "Bytes" => Ok(Value::String(raw.to_string())),
            _ => Err(ExecError::Runtime(format!(
                "env override not supported for type {name}"
            ))),
        }
    }

    fn is_optional_type(&self, ty: &TypeRef) -> bool {
        match &ty.kind {
            TypeRefKind::Optional(_) => true,
            TypeRefKind::Generic { base, .. } => base.name == "Option",
            _ => false,
        }
    }

    fn validate_value(&self, value: &Value, ty: &TypeRef, path: &str) -> ExecResult<()> {
        match &ty.kind {
            TypeRefKind::Optional(inner) => {
                if matches!(value, Value::Null) {
                    Ok(())
                } else {
                    self.validate_value(value, inner, path)
                }
            }
            TypeRefKind::Result { ok, err } => match value {
                Value::ResultOk(inner) => self.validate_value(inner, ok, path),
                Value::ResultErr(inner) => {
                    if let Some(err_ty) = err {
                        self.validate_value(inner, err_ty, path)
                    } else {
                        Ok(())
                    }
                }
                _ => Err(ExecError::Runtime(format!(
                    "type mismatch at {path}: expected Result"
                ))),
            },
            TypeRefKind::Refined { base, args } => {
                self.validate_simple(value, &base.name, path)?;
                self.check_refined(value, &base.name, args, path)
            }
            TypeRefKind::Simple(ident) => self.validate_simple(value, &ident.name, path),
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(ExecError::Runtime(
                            "Option expects 1 type argument".to_string(),
                        ));
                    }
                    if matches!(value, Value::Null) {
                        Ok(())
                    } else {
                        self.validate_value(value, &args[0], path)
                    }
                }
                "Result" => {
                    if args.len() != 2 {
                        return Err(ExecError::Runtime(
                            "Result expects 2 type arguments".to_string(),
                        ));
                    }
                    match value {
                        Value::ResultOk(inner) => self.validate_value(inner, &args[0], path),
                        Value::ResultErr(inner) => self.validate_value(inner, &args[1], path),
                        _ => Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Result"
                        ))),
                    }
                }
                _ => Err(ExecError::Runtime(format!(
                    "validation not supported for {}",
                    base.name
                ))),
            },
        }
    }

    fn validate_simple(&self, value: &Value, name: &str, path: &str) -> ExecResult<()> {
        let type_name = self.value_type_name(value);
        match name {
            "Int" => {
                if matches!(value, Value::Int(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Runtime(format!(
                        "type mismatch at {path}: expected Int, got {type_name}"
                    )))
                }
            }
            "Float" => {
                if matches!(value, Value::Float(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Runtime(format!(
                        "type mismatch at {path}: expected Float, got {type_name}"
                    )))
                }
            }
            "Bool" => {
                if matches!(value, Value::Bool(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Runtime(format!(
                        "type mismatch at {path}: expected Bool, got {type_name}"
                    )))
                }
            }
            "String" => {
                if matches!(value, Value::String(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Runtime(format!(
                        "type mismatch at {path}: expected String, got {type_name}"
                    )))
                }
            }
            "Id" => match value {
                Value::String(s) if !s.is_empty() => Ok(()),
                _ => Err(ExecError::Runtime(format!(
                    "type mismatch at {path}: expected Id, got {type_name}"
                ))),
            },
            "Email" => match value {
                Value::String(s) if rt_validate::is_email(s) => Ok(()),
                _ => Err(ExecError::Runtime(format!(
                    "type mismatch at {path}: expected Email, got {type_name}"
                ))),
            },
            "Bytes" => {
                if matches!(value, Value::String(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Runtime(format!(
                        "type mismatch at {path}: expected Bytes, got {type_name}"
                    )))
                }
            }
            _ => match value {
                Value::Struct { name: struct_name, .. } if struct_name == name => Ok(()),
                Value::Enum { name: enum_name, .. } if enum_name == name => Ok(()),
                _ => Err(ExecError::Runtime(format!(
                    "type mismatch at {path}: expected {name}, got {type_name}"
                ))),
            },
        }
    }

    fn value_type_name(&self, value: &Value) -> String {
        match value {
            Value::Unit => "Unit".to_string(),
            Value::Int(_) => "Int".to_string(),
            Value::Float(_) => "Float".to_string(),
            Value::Bool(_) => "Bool".to_string(),
            Value::String(_) => "String".to_string(),
            Value::Null => "Null".to_string(),
            Value::Struct { name, .. } => name.clone(),
            Value::Enum { name, .. } => name.clone(),
            Value::EnumCtor { name, .. } => name.clone(),
            Value::ResultOk(_) | Value::ResultErr(_) => "Result".to_string(),
            Value::Config(_) => "Config".to_string(),
            Value::Function(_) => "Function".to_string(),
            Value::Builtin(_) => "Builtin".to_string(),
        }
    }

    fn check_refined(&self, value: &Value, base: &str, args: &[Expr], path: &str) -> ExecResult<()> {
        match base {
            "String" => {
                let (min, max) = self.parse_length_range(args)?;
                let len = match value {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected String"
                        )))
                    }
                };
                if !rt_validate::check_len(len, min, max) {
                    let message = format!("length {len} out of range {min}..{max}");
                    return Err(ExecError::Error(self.validation_error_value(message)));
                }
                Ok(())
            }
            "Int" => {
                let (min, max) = self.parse_int_range(args)?;
                let val = match value {
                    Value::Int(v) => *v,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Int"
                        )))
                    }
                };
                if !rt_validate::check_int_range(val, min, max) {
                    let message = format!("value {val} out of range {min}..{max}");
                    return Err(ExecError::Error(self.validation_error_value(message)));
                }
                Ok(())
            }
            "Float" => {
                let (min, max) = self.parse_float_range(args)?;
                let val = match value {
                    Value::Float(v) => *v,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Float"
                        )))
                    }
                };
                if !rt_validate::check_float_range(val, min, max) {
                    let message = format!("value {val} out of range {min}..{max}");
                    return Err(ExecError::Error(self.validation_error_value(message)));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn parse_length_range(&self, args: &[Expr]) -> ExecResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_int_range(&self, args: &[Expr]) -> ExecResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_float_range(&self, args: &[Expr]) -> ExecResult<(f64, f64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_f64(left)
            .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_f64(right)
            .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn extract_range_args<'b>(&self, args: &'b [Expr]) -> ExecResult<(&'b Expr, &'b Expr)> {
        if args.len() == 1 {
            if let ExprKind::Binary {
                op: BinaryOp::Range,
                left,
                right,
            } = &args[0].kind
            {
                return Ok((left, right));
            }
        }
        if args.len() == 2 {
            return Ok((&args[0], &args[1]));
        }
        Err(ExecError::Runtime(
            "refined types expect a range like 1..10".to_string(),
        ))
    }

    fn literal_to_i64(&self, expr: &Expr) -> Option<i64> {
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

    fn literal_to_f64(&self, expr: &Expr) -> Option<f64> {
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

    fn validation_error_value(&self, message: impl Into<String>) -> Value {
        let mut fields = HashMap::new();
        fields.insert("message".to_string(), Value::String(message.into()));
        Value::Struct {
            name: "ValidationError".to_string(),
            fields,
        }
    }

    fn default_error_value(&self, message: impl Into<String>) -> Value {
        let mut fields = HashMap::new();
        fields.insert("message".to_string(), Value::String(message.into()));
        Value::Struct {
            name: "Error".to_string(),
            fields,
        }
    }

    // email validation lives in fuse-rt
}
