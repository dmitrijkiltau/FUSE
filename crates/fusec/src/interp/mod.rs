use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use fuse_rt::{config as rt_config, error as rt_error, json as rt_json, validate as rt_validate};

use crate::ast::{
    AppDecl, BinaryOp, Block, ConfigDecl, EnumDecl, Expr, ExprKind, FnDecl, HttpVerb, Ident,
    InterpPart, Item, Literal, Pattern, PatternField, PatternKind, Program, RouteDecl, ServiceDecl,
    Stmt, StmtKind, StructField, TypeDecl, TypeRef, TypeRefKind, UnaryOp,
};

#[derive(Clone, Debug)]
pub enum Value {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Null,
    List(Vec<Value>),
    Map(HashMap<String, Value>),
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
            Value::List(items) => {
                let text = items
                    .iter()
                    .map(|item| item.to_string_value())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{text}]")
            }
            Value::Map(items) => {
                let text = items
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.to_string_value()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{text}}}")
            }
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
    if let Some(json) = error_json_for_value(value) {
        return rt_json::encode(&json);
    }
    value.to_string_value()
}

fn error_json_for_value(value: &Value) -> Option<rt_json::JsonValue> {
    let Value::Struct { name, fields } = value else {
        return None;
    };
    match name.as_str() {
        "ValidationError" => {
            let message = match fields.get("message") {
                Some(Value::String(msg)) => msg.as_str(),
                _ => "validation failed",
            };
            let field_items = extract_validation_fields(fields.get("fields"));
            Some(rt_error::validation_error_json(message, &field_items))
        }
        "Error" => {
            let code = match fields.get("code") {
                Some(Value::String(code)) => code.as_str(),
                _ => "error",
            };
            let message = match fields.get("message") {
                Some(Value::String(msg)) => msg.as_str(),
                _ => "error",
            };
            Some(rt_error::error_json(code, message, None))
        }
        other => {
            let (code, default_message) = builtin_error_defaults(other)?;
            let message = match fields.get("message") {
                Some(Value::String(msg)) => msg.as_str(),
                _ => default_message,
            };
            Some(rt_error::error_json(code, message, None))
        }
    }
}

fn builtin_error_defaults(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "BadRequest" => Some(("bad_request", "bad request")),
        "Unauthorized" => Some(("unauthorized", "unauthorized")),
        "Forbidden" => Some(("forbidden", "forbidden")),
        "NotFound" => Some(("not_found", "not found")),
        "Conflict" => Some(("conflict", "conflict")),
        _ => None,
    }
}

fn extract_validation_fields(value: Option<&Value>) -> Vec<rt_error::ValidationField> {
    let mut out = Vec::new();
    let Some(Value::List(items)) = value else {
        return out;
    };
    for item in items {
        let Value::Struct { fields, .. } = item else { continue };
        let Some(Value::String(path)) = fields.get("path") else { continue };
        let Some(Value::String(code)) = fields.get("code") else { continue };
        let Some(Value::String(message)) = fields.get("message") else { continue };
        out.push(rt_error::ValidationField {
            path: path.clone(),
            code: code.clone(),
            message: message.clone(),
        });
    }
    out
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
    services: HashMap<String, &'a ServiceDecl>,
    types: HashMap<String, &'a TypeDecl>,
    config_decls: HashMap<String, &'a ConfigDecl>,
    enums: HashMap<String, &'a EnumDecl>,
}

impl<'a> Interpreter<'a> {
    pub fn new(program: &'a Program) -> Self {
        let mut functions = HashMap::new();
        let mut apps = Vec::new();
        let mut services = HashMap::new();
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
                Item::Service(decl) => {
                    services.insert(decl.name.name.clone(), decl);
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
            services,
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

    pub fn parse_cli_value(&self, ty: &TypeRef, raw: &str) -> Result<Value, String> {
        match self.parse_env_value(ty, raw) {
            Ok(value) => Ok(value),
            Err(err) => Err(self.render_exec_error(err)),
        }
    }

    pub fn call_function_with_named_args(
        &mut self,
        name: &str,
        args: &HashMap<String, Value>,
    ) -> Result<Value, String> {
        let decl = match self.functions.get(name) {
            Some(decl) => *decl,
            None => return Err(format!("unknown function {name}")),
        };
        self.env.push();
        for param in &decl.params {
            let value = if let Some(value) = args.get(&param.name.name) {
                value.clone()
            } else if let Some(default) = &param.default {
                match self.eval_expr(default) {
                    Ok(value) => value,
                    Err(err) => {
                        self.env.pop();
                        return Err(self.render_exec_error(err));
                    }
                }
            } else if self.is_optional_type(&param.ty) {
                Value::Null
            } else {
                self.env.pop();
                return Err(format!("missing argument {}", param.name.name));
            };
            if let Err(err) = self.validate_value(&value, &param.ty, &param.name.name) {
                self.env.pop();
                return Err(self.render_exec_error(err));
            }
            self.env.insert(&param.name.name, value);
        }
        let result = self.eval_block(&decl.body);
        self.env.pop();
        match result {
            Ok(value) => Ok(value),
            Err(ExecError::Return(value)) => Ok(value),
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
        let config_path =
            std::env::var("FUSE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        let file_values =
            rt_config::load_config_file(&config_path).map_err(ExecError::Runtime)?;
        for item in &self.program.items {
            if let Item::Config(decl) = item {
                let name = decl.name.name.clone();
                self.configs.insert(name.clone(), HashMap::new());
                let section = file_values.get(&name);
                for field in &decl.fields {
                    let key = self.config_env_key(&decl.name.name, &field.name.name);
                    let path = format!("{}.{}", decl.name.name, field.name.name);
                    let value = match std::env::var(&key) {
                        Ok(raw) => {
                            let value = self
                                .parse_env_value(&field.ty, &raw)
                                .map_err(|err| self.map_parse_error(err, &path))?;
                            self.validate_value(&value, &field.ty, &path)?;
                            value
                        }
                        Err(_) => {
                            let value = if let Some(section) = section {
                                if let Some(raw) = section.get(&field.name.name) {
                                    self.parse_env_value(&field.ty, raw)
                                        .map_err(|err| self.map_parse_error(err, &path))?
                                } else {
                                    self.eval_expr(&field.value)?
                                }
                            } else {
                                self.eval_expr(&field.value)?
                            };
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

    fn map_parse_error(&self, err: ExecError, path: &str) -> ExecError {
        match err {
            ExecError::Runtime(message) => {
                ExecError::Error(self.validation_error_value(path, "invalid_value", message))
            }
            other => other,
        }
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
            ExprKind::ListLit(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval_expr(item)?);
                }
                Ok(Value::List(values))
            }
            ExprKind::MapLit(pairs) => {
                let mut values = HashMap::new();
                for (key_expr, value_expr) in pairs {
                    let key_val = self.eval_expr(key_expr)?;
                    let key = match &key_val {
                        Value::String(text) => text.clone(),
                        _ => {
                            return Err(ExecError::Runtime(format!(
                                "map keys must be strings, got {}",
                                self.value_type_name(&key_val)
                            )))
                        }
                    };
                    let value = self.eval_expr(value_expr)?;
                    values.insert(key, value);
                }
                Ok(Value::Map(values))
            }
            ExprKind::InterpString(parts) => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        InterpPart::Text(text) => out.push_str(text),
                        InterpPart::Expr(expr) => {
                            let value = self.eval_expr(expr)?;
                            out.push_str(&value.to_string_value());
                        }
                    }
                }
                Ok(Value::String(out))
            }
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
            "serve" => {
                let port = match args.get(0) {
                    Some(Value::Int(v)) => *v,
                    Some(Value::Float(v)) => *v as i64,
                    Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
                    _ => {
                        return Err(ExecError::Runtime(
                            "serve expects a port number".to_string(),
                        ))
                    }
                };
                self.eval_serve(port)
            }
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

    fn eval_serve(&mut self, port: i64) -> ExecResult<Value> {
        let service = self.select_service()?.clone();
        let host = std::env::var("FUSE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = port
            .try_into()
            .map_err(|_| ExecError::Runtime("invalid port".to_string()))?;
        let addr = format!("{host}:{port}");
        let listener = TcpListener::bind(&addr)
            .map_err(|err| ExecError::Runtime(format!("failed to bind {addr}: {err}")))?;
        let max_requests = std::env::var("FUSE_MAX_REQUESTS")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(0);
        let mut handled = 0usize;
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    return Err(ExecError::Runtime(format!(
                        "failed to accept connection: {err}"
                    )))
                }
            };
            let response = match self.handle_http_request(&service, &mut stream) {
                Ok(resp) => resp,
                Err(err) => self.http_error_response(err),
            };
            let _ = stream.write_all(response.as_bytes());
            handled += 1;
            if max_requests > 0 && handled >= max_requests {
                break;
            }
        }
        Ok(Value::Unit)
    }

    fn select_service(&self) -> ExecResult<&'a ServiceDecl> {
        if self.services.is_empty() {
            return Err(ExecError::Runtime("no service declared".to_string()));
        }
        if let Ok(name) = std::env::var("FUSE_SERVICE") {
            return self
                .services
                .get(&name)
                .copied()
                .ok_or_else(|| ExecError::Runtime(format!("service not found: {name}")));
        }
        if self.services.len() == 1 {
            return Ok(*self.services.values().next().unwrap());
        }
        Err(ExecError::Runtime(
            "multiple services declared; set FUSE_SERVICE".to_string(),
        ))
    }

    fn handle_http_request(
        &mut self,
        service: &ServiceDecl,
        stream: &mut TcpStream,
    ) -> ExecResult<String> {
        let request = self.read_http_request(stream)?;
        let verb = match request.method.as_str() {
            "GET" => HttpVerb::Get,
            "POST" => HttpVerb::Post,
            "PUT" => HttpVerb::Put,
            "PATCH" => HttpVerb::Patch,
            "DELETE" => HttpVerb::Delete,
            _ => {
                return Ok(self.http_response(
                    405,
                    self.internal_error_json("method not allowed"),
                ))
            }
        };
        let path = request
            .path
            .split('?')
            .next()
            .unwrap_or(&request.path)
            .to_string();
        let (route, params) = match self.match_route(service, &verb, &path)? {
            Some(result) => result,
            None => {
                let body = self.error_json_from_code("not_found", "not found");
                return Ok(self.http_response(404, body));
            }
        };
        let body_value = if let Some(body_ty) = &route.body_type {
            let body_text = String::from_utf8_lossy(&request.body);
            if body_text.trim().is_empty() {
                return Err(ExecError::Error(self.validation_error_value(
                    "body",
                    "missing_field",
                    "missing JSON body",
                )));
            }
            let json = rt_json::decode(&body_text).map_err(|msg| {
                ExecError::Error(self.validation_error_value("body", "invalid_json", msg))
            })?;
            Some(self.decode_json_value(&json, body_ty, "body")?)
        } else {
            None
        };
        let value = match self.eval_route(route, params, body_value) {
            Ok(value) => value,
            Err(err) => return Err(err),
        };
        match value {
            Value::ResultErr(err) => {
                let status = self.http_status_for_error_value(&err);
                let json = self.error_json_from_value(&err);
                Ok(self.http_response(status, json))
            }
            Value::ResultOk(ok) => {
                let json = self.value_to_json(&ok);
                Ok(self.http_response(200, rt_json::encode(&json)))
            }
            other => {
                let json = self.value_to_json(&other);
                Ok(self.http_response(200, rt_json::encode(&json)))
            }
        }
    }

    fn eval_route(
        &mut self,
        route: &RouteDecl,
        params: HashMap<String, Value>,
        body_value: Option<Value>,
    ) -> ExecResult<Value> {
        self.env.push();
        for (name, value) in params {
            self.env.insert(&name, value);
        }
        if let Some(body) = body_value {
            self.env.insert("body", body);
        }
        let result = self.eval_block(&route.body);
        self.env.pop();
        match result {
            Ok(value) => Ok(value),
            Err(ExecError::Return(value)) => Ok(value),
            Err(err) => Err(err),
        }
    }

    fn match_route<'r>(
        &mut self,
        service: &'r ServiceDecl,
        verb: &HttpVerb,
        path: &str,
    ) -> ExecResult<Option<(&'r RouteDecl, HashMap<String, Value>)>> {
        let base_segments = split_path(&service.base_path.value);
        let req_segments = split_path(path);
        if req_segments.len() < base_segments.len()
            || req_segments[..base_segments.len()] != base_segments[..]
        {
            return Ok(None);
        }
        let req_segments = &req_segments[base_segments.len()..];
        for route in &service.routes {
            if &route.verb != verb {
                continue;
            }
            let route_segments = split_path(&route.path.value);
            if route_segments.len() != req_segments.len() {
                continue;
            }
            let mut params = HashMap::new();
            let mut matched = true;
            for (seg, req) in route_segments.iter().zip(req_segments.iter()) {
                if let Some((name, ty_name)) = parse_route_param(seg) {
                    let ty = TypeRef {
                        kind: TypeRefKind::Simple(Ident {
                            name: ty_name.to_string(),
                            span: crate::span::Span::default(),
                        }),
                        span: crate::span::Span::default(),
                    };
                    let value = self
                        .parse_env_value(&ty, req)
                        .map_err(|err| self.map_parse_error(err, &name))?;
                    self.validate_value(&value, &ty, &name)?;
                    params.insert(name, value);
                } else if seg != req {
                    matched = false;
                    break;
                }
            }
            if matched {
                return Ok(Some((route, params)));
            }
        }
        Ok(None)
    }

    fn read_http_request(&self, stream: &mut TcpStream) -> ExecResult<HttpRequest> {
        let mut buffer = Vec::new();
        let mut temp = [0u8; 1024];
        let mut header_end = None;
        loop {
            let read = stream
                .read(&mut temp)
                .map_err(|err| ExecError::Runtime(format!("failed to read request: {err}")))?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..read]);
            if let Some(pos) = find_header_end(&buffer) {
                header_end = Some(pos);
                break;
            }
            if buffer.len() > 1024 * 1024 {
                return Err(ExecError::Runtime("request header too large".to_string()));
            }
        }
        let header_end = header_end.ok_or_else(|| {
            ExecError::Runtime("invalid HTTP request: missing headers".to_string())
        })?;
        let header_bytes = &buffer[..header_end];
        let header_text = String::from_utf8_lossy(header_bytes);
        let mut lines = header_text.split("\r\n");
        let request_line = lines
            .next()
            .ok_or_else(|| ExecError::Runtime("invalid HTTP request line".to_string()))?;
        let mut parts = request_line.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| ExecError::Runtime("invalid HTTP request line".to_string()))?
            .to_string();
        let path = parts
            .next()
            .ok_or_else(|| ExecError::Runtime("invalid HTTP request line".to_string()))?
            .to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        let content_length = headers
            .get("content-length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = buffer[header_end + 4..].to_vec();
        while body.len() < content_length {
            let read = stream
                .read(&mut temp)
                .map_err(|err| ExecError::Runtime(format!("failed to read body: {err}")))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&temp[..read]);
        }
        if body.len() > content_length {
            body.truncate(content_length);
        }
        Ok(HttpRequest { method, path, body })
    }

    fn http_response(&self, status: u16, body: String) -> String {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            409 => "Conflict",
            500 => "Internal Server Error",
            _ => "OK",
        };
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    fn http_error_response(&self, err: ExecError) -> String {
        match err {
            ExecError::Error(value) => {
                let status = self.http_status_for_error_value(&value);
                let body = self.error_json_from_value(&value);
                self.http_response(status, body)
            }
            ExecError::Runtime(message) => {
                let body = self.internal_error_json(&message);
                self.http_response(500, body)
            }
            ExecError::Return(_) => {
                self.http_response(500, self.internal_error_json("unexpected return"))
            }
        }
    }

    fn http_status_for_error_value(&self, value: &Value) -> u16 {
        match value {
            Value::Struct { name, fields } => match name.as_str() {
                "ValidationError" => 400,
                "BadRequest" => 400,
                "Unauthorized" => 401,
                "Forbidden" => 403,
                "NotFound" => 404,
                "Conflict" => 409,
                "Error" => fields
                    .get("status")
                    .and_then(|v| match v {
                        Value::Int(n) => (*n).try_into().ok(),
                        _ => None,
                    })
                    .unwrap_or(500),
                _ => 500,
            },
            _ => 500,
        }
    }

    fn error_json_from_value(&self, value: &Value) -> String {
        if let Some(json) = error_json_for_value(value) {
            return rt_json::encode(&json);
        }
        self.internal_error_json("internal error")
    }

    fn error_json_from_code(&self, code: &str, message: &str) -> String {
        rt_json::encode(&rt_error::error_json(code, message, None))
    }

    fn internal_error_json(&self, message: &str) -> String {
        self.error_json_from_code("internal_error", message)
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
                _ => Err(ExecError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!(
                        "expected Result, got {}",
                        self.value_type_name(value)
                    ),
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
                        _ => Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected Result, got {}",
                                self.value_type_name(value)
                            ),
                        ))),
                    }
                }
                "List" => {
                    if args.len() != 1 {
                        return Err(ExecError::Runtime(
                            "List expects 1 type argument".to_string(),
                        ));
                    }
                    match value {
                        Value::List(items) => {
                            for (idx, item) in items.iter().enumerate() {
                                let item_path = format!("{path}[{idx}]");
                                self.validate_value(item, &args[0], &item_path)?;
                            }
                            Ok(())
                        }
                        _ => Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected List, got {}",
                                self.value_type_name(value)
                            ),
                        ))),
                    }
                }
                "Map" => {
                    if args.len() != 2 {
                        return Err(ExecError::Runtime(
                            "Map expects 2 type arguments".to_string(),
                        ));
                    }
                    match value {
                        Value::Map(items) => {
                            for (key, val) in items.iter() {
                                let key_value = Value::String(key.clone());
                                let key_path = format!("{path}.{key}");
                                self.validate_value(&key_value, &args[0], &key_path)?;
                                self.validate_value(val, &args[1], &key_path)?;
                            }
                            Ok(())
                        }
                        _ => Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected Map, got {}",
                                self.value_type_name(value)
                            ),
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
                        "expected Int, got {type_name}"
                    )))
                }
            }
            "Float" => {
                if matches!(value, Value::Float(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Float, got {type_name}"),
                    )))
                }
            }
            "Bool" => {
                if matches!(value, Value::Bool(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bool, got {type_name}"),
                    )))
                }
            }
            "String" => {
                if matches!(value, Value::String(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected String, got {type_name}"),
                    )))
                }
            }
            "Id" => match value {
                Value::String(s) if !s.is_empty() => Ok(()),
                Value::String(_) => Err(ExecError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    "expected non-empty Id".to_string(),
                ))),
                _ => Err(ExecError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Id, got {type_name}"),
                ))),
            },
            "Email" => match value {
                Value::String(s) if rt_validate::is_email(s) => Ok(()),
                Value::String(_) => Err(ExecError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    "invalid email address".to_string(),
                ))),
                _ => Err(ExecError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Email, got {type_name}"),
                ))),
            },
            "Bytes" => {
                if matches!(value, Value::String(_)) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bytes, got {type_name}"),
                    )))
                }
            }
            _ => match value {
                Value::Struct { name: struct_name, .. } if struct_name == name => Ok(()),
                Value::Enum { name: enum_name, .. } if enum_name == name => Ok(()),
                _ => Err(ExecError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected {name}, got {type_name}"),
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
            Value::List(_) => "List".to_string(),
            Value::Map(_) => "Map".to_string(),
            Value::Struct { name, .. } => name.clone(),
            Value::Enum { name, .. } => name.clone(),
            Value::EnumCtor { name, .. } => name.clone(),
            Value::ResultOk(_) | Value::ResultErr(_) => "Result".to_string(),
            Value::Config(_) => "Config".to_string(),
            Value::Function(_) => "Function".to_string(),
            Value::Builtin(_) => "Builtin".to_string(),
        }
    }

    fn value_to_json(&self, value: &Value) -> rt_json::JsonValue {
        match value {
            Value::Unit => rt_json::JsonValue::Null,
            Value::Int(v) => rt_json::JsonValue::Number(*v as f64),
            Value::Float(v) => rt_json::JsonValue::Number(*v),
            Value::Bool(v) => rt_json::JsonValue::Bool(*v),
            Value::String(v) => rt_json::JsonValue::String(v.clone()),
            Value::Null => rt_json::JsonValue::Null,
            Value::List(items) => {
                rt_json::JsonValue::Array(items.iter().map(|v| self.value_to_json(v)).collect())
            }
            Value::Map(items) => {
                let mut out = BTreeMap::new();
                for (key, value) in items {
                    out.insert(key.clone(), self.value_to_json(value));
                }
                rt_json::JsonValue::Object(out)
            }
            Value::Struct { fields, .. } => {
                let mut out = BTreeMap::new();
                for (key, value) in fields {
                    out.insert(key.clone(), self.value_to_json(value));
                }
                rt_json::JsonValue::Object(out)
            }
            Value::Enum {
                variant, payload, ..
            } => {
                let mut out = BTreeMap::new();
                out.insert(
                    "type".to_string(),
                    rt_json::JsonValue::String(variant.clone()),
                );
                match payload.len() {
                    0 => {}
                    1 => {
                        out.insert("data".to_string(), self.value_to_json(&payload[0]));
                    }
                    _ => {
                        let items = payload.iter().map(|v| self.value_to_json(v)).collect();
                        out.insert("data".to_string(), rt_json::JsonValue::Array(items));
                    }
                }
                rt_json::JsonValue::Object(out)
            }
            Value::ResultOk(value) => self.value_to_json(value),
            Value::ResultErr(value) => self.value_to_json(value),
            Value::Config(name) => rt_json::JsonValue::String(name.clone()),
            Value::Function(name) => rt_json::JsonValue::String(name.clone()),
            Value::Builtin(name) => rt_json::JsonValue::String(name.clone()),
            Value::EnumCtor { name, variant } => {
                rt_json::JsonValue::String(format!("{name}.{variant}"))
            }
        }
    }

    fn decode_json_value(
        &mut self,
        json: &rt_json::JsonValue,
        ty: &TypeRef,
        path: &str,
    ) -> ExecResult<Value> {
        let value = match &ty.kind {
            TypeRefKind::Optional(inner) => {
                if matches!(json, rt_json::JsonValue::Null) {
                    Value::Null
                } else {
                    self.decode_json_value(json, inner, path)?
                }
            }
            TypeRefKind::Refined { base, .. } => {
                let base_ty = TypeRef {
                    kind: TypeRefKind::Simple(base.clone()),
                    span: ty.span,
                };
                let value = self.decode_json_value(json, &base_ty, path)?;
                self.validate_value(&value, ty, path)?;
                return Ok(value);
            }
            TypeRefKind::Simple(ident) => {
                if let Some(value) = self.decode_simple_json(json, &ident.name, path)? {
                    value
                } else if self.types.contains_key(&ident.name) {
                    self.decode_struct_json(json, &ident.name, path)?
                } else if self.enums.contains_key(&ident.name) {
                    self.decode_enum_json(json, &ident.name, path)?
                } else {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("unknown type {}", ident.name),
                    )));
                }
            }
            TypeRefKind::Result { .. } => {
                return Err(ExecError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    "Result is not supported for JSON body",
                )))
            }
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(ExecError::Runtime(
                            "Option expects 1 type argument".to_string(),
                        ));
                    }
                    if matches!(json, rt_json::JsonValue::Null) {
                        Value::Null
                    } else {
                        self.decode_json_value(json, &args[0], path)?
                    }
                }
                "Result" => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        "Result is not supported for JSON body",
                    )))
                }
                "List" => {
                    if args.len() != 1 {
                        return Err(ExecError::Runtime(
                            "List expects 1 type argument".to_string(),
                        ));
                    }
                    let rt_json::JsonValue::Array(items) = json else {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            "expected List",
                        )));
                    };
                    let mut values = Vec::with_capacity(items.len());
                    for (idx, item) in items.iter().enumerate() {
                        let item_path = format!("{path}[{idx}]");
                        values.push(self.decode_json_value(item, &args[0], &item_path)?);
                    }
                    Value::List(values)
                }
                "Map" => {
                    if args.len() != 2 {
                        return Err(ExecError::Runtime(
                            "Map expects 2 type arguments".to_string(),
                        ));
                    }
                    let rt_json::JsonValue::Object(items) = json else {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            "expected Map",
                        )));
                    };
                    let mut values = HashMap::new();
                    for (key, item) in items.iter() {
                        let key_value = Value::String(key.clone());
                        let key_path = format!("{path}.{key}");
                        self.validate_value(&key_value, &args[0], &key_path)?;
                        let value = self.decode_json_value(item, &args[1], &key_path)?;
                        values.insert(key.clone(), value);
                    }
                    Value::Map(values)
                }
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("unsupported type {}", base.name),
                    )))
                }
            },
        };
        self.validate_value(&value, ty, path)?;
        Ok(value)
    }

    fn decode_simple_json(
        &self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> ExecResult<Option<Value>> {
        let value = match name {
            "Int" => match json {
                rt_json::JsonValue::Number(n) if n.fract() == 0.0 => Value::Int(*n as i64),
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Int",
                    )))
                }
            },
            "Float" => match json {
                rt_json::JsonValue::Number(n) => Value::Float(*n),
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Float",
                    )))
                }
            },
            "Bool" => match json {
                rt_json::JsonValue::Bool(v) => Value::Bool(*v),
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Bool",
                    )))
                }
            },
            "String" | "Id" | "Email" | "Bytes" => match json {
                rt_json::JsonValue::String(v) => Value::String(v.clone()),
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected String",
                    )))
                }
            },
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    fn decode_struct_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> ExecResult<Value> {
        let rt_json::JsonValue::Object(map) = json else {
            return Err(ExecError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("expected {name}"),
            )));
        };
        let decl = self.types.get(name).ok_or_else(|| {
            ExecError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("unknown type {name}"),
            ))
        })?;
        let fields = decl.fields.clone();
        let mut values = HashMap::new();
        for (key, value) in map {
            let field = fields.iter().find(|f| f.name.name == *key);
            let Some(field_decl) = field else {
                return Err(ExecError::Error(self.validation_error_value(
                    &format!("{path}.{key}"),
                    "unknown_field",
                    "unknown field",
                )));
            };
            let field_path = format!("{path}.{key}");
            let decoded = self.decode_json_value(value, &field_decl.ty, &field_path)?;
            values.insert(key.clone(), decoded);
        }
        for field_decl in &fields {
            if values.contains_key(&field_decl.name.name) {
                continue;
            }
            let field_path = format!("{path}.{}", field_decl.name.name);
            if let Some(default) = &field_decl.default {
                let value = self.eval_expr(default)?;
                self.validate_value(&value, &field_decl.ty, &field_path)?;
                values.insert(field_decl.name.name.clone(), value);
            } else if self.is_optional_type(&field_decl.ty) {
                values.insert(field_decl.name.name.clone(), Value::Null);
            } else {
                return Err(ExecError::Error(self.validation_error_value(
                    &field_path,
                    "missing_field",
                    "missing field",
                )));
            }
        }
        Ok(Value::Struct {
            name: name.to_string(),
            fields: values,
        })
    }

    fn decode_enum_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> ExecResult<Value> {
        let rt_json::JsonValue::Object(map) = json else {
            return Err(ExecError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("expected {name}"),
            )));
        };
        let Some(rt_json::JsonValue::String(variant_name)) = map.get("type") else {
            return Err(ExecError::Error(self.validation_error_value(
                path,
                "missing_field",
                "missing enum type",
            )));
        };
        let decl = self.enums.get(name).ok_or_else(|| {
            ExecError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("unknown enum {name}"),
            ))
        })?;
        let variants = decl.variants.clone();
        let variant = variants
            .iter()
            .find(|v| v.name.name == *variant_name)
            .ok_or_else(|| {
                ExecError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    format!("unknown variant {variant_name}"),
                ))
            })?;
        let payload = if variant.payload.is_empty() {
            Vec::new()
        } else {
            let data = map.get("data").ok_or_else(|| {
                ExecError::Error(self.validation_error_value(
                    path,
                    "missing_field",
                    "missing enum data",
                ))
            })?;
            if variant.payload.len() == 1 {
                vec![self.decode_json_value(
                    data,
                    &variant.payload[0],
                    &format!("{path}.data"),
                )?]
            } else {
                let rt_json::JsonValue::Array(items) = data else {
                    return Err(ExecError::Error(self.validation_error_value(
                        &format!("{path}.data"),
                        "type_mismatch",
                        "expected enum payload array",
                    )));
                };
                if items.len() != variant.payload.len() {
                    return Err(ExecError::Error(self.validation_error_value(
                        &format!("{path}.data"),
                        "invalid_value",
                        "enum payload length mismatch",
                    )));
                }
                let mut out = Vec::new();
                for (idx, (item, ty)) in items.iter().zip(variant.payload.iter()).enumerate() {
                    out.push(self.decode_json_value(
                        item,
                        ty,
                        &format!("{path}.data[{idx}]"),
                    )?);
                }
                out
            }
        };
        Ok(Value::Enum {
            name: name.to_string(),
            variant: variant_name.clone(),
            payload,
        })
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
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        message,
                    )));
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
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        message,
                    )));
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
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        message,
                    )));
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

    fn validation_error_value(&self, path: &str, code: &str, message: impl Into<String>) -> Value {
        let field = self.validation_field_value(path, code, message);
        let mut fields = HashMap::new();
        fields.insert(
            "message".to_string(),
            Value::String("validation failed".to_string()),
        );
        fields.insert("fields".to_string(), Value::List(vec![field]));
        Value::Struct {
            name: "ValidationError".to_string(),
            fields,
        }
    }

    fn validation_field_value(&self, path: &str, code: &str, message: impl Into<String>) -> Value {
        let mut fields = HashMap::new();
        fields.insert("path".to_string(), Value::String(path.to_string()));
        fields.insert("code".to_string(), Value::String(code.to_string()));
        fields.insert("message".to_string(), Value::String(message.into()));
        Value::Struct {
            name: "ValidationField".to_string(),
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

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn split_path(path: &str) -> Vec<String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').map(|s| s.to_string()).collect()
    }
}

fn parse_route_param(segment: &str) -> Option<(String, String)> {
    if !segment.starts_with('{') || !segment.ends_with('}') {
        return None;
    }
    let inner = &segment[1..segment.len() - 1];
    let mut parts = inner.splitn(2, ':');
    let name = parts.next().unwrap_or("").trim();
    let ty = parts.next().unwrap_or("").trim();
    if name.is_empty() || ty.is_empty() {
        return None;
    }
    Some((name.to_string(), ty.to_string()))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
}
