use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use fuse_rt::{
    bytes as rt_bytes, config as rt_config, error as rt_error, json as rt_json,
    validate as rt_validate,
};

use crate::ast::{
    AppDecl, BinaryOp, Block, ConfigDecl, EnumDecl, Expr, ExprKind, FnDecl, HttpVerb, Ident,
    InterpPart, Item, Literal, MigrationDecl, Pattern, PatternField, PatternKind, Program,
    RouteDecl, ServiceDecl, Stmt, StmtKind, StructField, TestDecl, TypeDecl, TypeRef, TypeRefKind,
    UnaryOp,
};
use crate::db::{Db, Query};
use crate::loader::{ModuleId, ModuleMap, ModuleRegistry};

#[derive(Clone, Debug)]
pub enum Value {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    Html(HtmlNode),
    Null,
    List(Vec<Value>),
    Map(HashMap<String, Value>),
    Boxed(Rc<RefCell<Value>>),
    Query(Query),
    Task(Task),
    Iterator(IteratorValue),
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

#[derive(Clone, Debug)]
pub enum HtmlNode {
    Element {
        tag: String,
        attrs: HashMap<String, String>,
        children: Vec<HtmlNode>,
    },
    Text(String),
    Raw(String),
}

impl HtmlNode {
    pub fn render_to_string(&self) -> String {
        let mut out = String::new();
        self.render_into(&mut out);
        out
    }

    fn render_into(&self, out: &mut String) {
        match self {
            HtmlNode::Text(text) | HtmlNode::Raw(text) => {
                out.push_str(text);
            }
            HtmlNode::Element {
                tag,
                attrs,
                children,
            } => {
                out.push('<');
                out.push_str(tag);
                let mut attrs_sorted: Vec<(&String, &String)> = attrs.iter().collect();
                attrs_sorted.sort_by(|(left, _), (right, _)| left.cmp(right));
                for (key, value) in attrs_sorted {
                    out.push(' ');
                    out.push_str(key);
                    out.push_str("=\"");
                    out.push_str(value);
                    out.push('"');
                }
                out.push('>');
                for child in children {
                    child.render_into(out);
                }
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
    }
}

#[derive(Clone, Debug)]
enum AssignStep {
    Field { name: String, optional: bool },
    Index { key: Value, optional: bool },
}

impl AssignStep {
    fn is_optional(&self) -> bool {
        match self {
            AssignStep::Field { optional, .. } | AssignStep::Index { optional, .. } => *optional,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Task {
    state: Rc<RefCell<TaskState>>,
}

#[derive(Clone, Debug)]
pub struct IteratorValue {
    pub values: Vec<Value>,
    pub index: usize,
}

impl IteratorValue {
    pub fn new(values: Vec<Value>) -> Self {
        Self { values, index: 0 }
    }
}

#[derive(Clone, Debug)]
struct TaskState {
    id: u64,
    done: bool,
    cancelled: bool,
    result: TaskResult,
}

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(crate) enum TaskResult {
    Ok(Value),
    Error(Value),
    Runtime(String),
}

impl Task {
    pub(crate) fn from_task_result(result: TaskResult) -> Self {
        Task::from_state(
            NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed),
            true,
            false,
            result,
        )
    }

    pub(crate) fn from_state(id: u64, done: bool, cancelled: bool, result: TaskResult) -> Self {
        Task {
            state: Rc::new(RefCell::new(TaskState {
                id,
                done,
                cancelled,
                result,
            })),
        }
    }

    fn new(result: ExecResult<Value>) -> Self {
        let result = match result {
            Ok(value) => TaskResult::Ok(value),
            Err(ExecError::Return(value)) => TaskResult::Ok(value),
            Err(ExecError::Error(value)) => TaskResult::Error(value),
            Err(ExecError::Runtime(message)) => TaskResult::Runtime(message),
            Err(ExecError::Break) => TaskResult::Runtime("break outside of loop".to_string()),
            Err(ExecError::Continue) => TaskResult::Runtime("continue outside of loop".to_string()),
        };
        Task::from_task_result(result)
    }

    pub(crate) fn result_raw(&self) -> TaskResult {
        self.state.borrow().result.clone()
    }

    pub fn id(&self) -> u64 {
        self.state.borrow().id
    }

    pub fn is_done(&self) -> bool {
        self.state.borrow().done
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.state.borrow().cancelled
    }

    pub fn cancel(&self) -> bool {
        let mut state = self.state.borrow_mut();
        if state.done || state.cancelled {
            return false;
        }
        state.cancelled = true;
        true
    }

    fn result(&self) -> ExecResult<Value> {
        match self.result_raw() {
            TaskResult::Ok(value) => Ok(value),
            TaskResult::Error(value) => Err(ExecError::Error(value)),
            TaskResult::Runtime(message) => Err(ExecError::Runtime(message)),
        }
    }
}

impl Value {
    pub fn to_string_value(&self) -> String {
        match self.unboxed() {
            Value::Unit => "()".to_string(),
            Value::Int(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Bool(v) => v.to_string(),
            Value::String(v) => v.clone(),
            Value::Bytes(v) => rt_bytes::encode_base64(&v),
            Value::Html(node) => node.render_to_string(),
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
            Value::Boxed(_) => "<box>".to_string(),
            Value::Query(_) => "<query>".to_string(),
            Value::Task(_) => "<task>".to_string(),
            Value::Iterator(_) => "<iterator>".to_string(),
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

    pub(crate) fn unboxed(&self) -> Value {
        match self {
            Value::Boxed(cell) => cell.borrow().unboxed(),
            other => other.clone(),
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
    let value = value.unboxed();
    let Value::Struct { name, fields } = value else {
        return None;
    };
    let name = name.as_str();
    match name {
        "std.Error.Validation" | "Validation" => {
            let message = match fields.get("message") {
                Some(Value::String(msg)) => msg.as_str(),
                _ => "validation failed",
            };
            let field_items = extract_validation_fields(fields.get("fields"));
            Some(rt_error::validation_error_json(message, &field_items))
        }
        "std.Error" | "Error" => {
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

fn split_type_name(name: &str) -> (Option<&str>, &str) {
    if name.starts_with("std.") {
        return (None, name);
    }
    match name.split_once('.') {
        Some((module, rest)) if !module.is_empty() && !rest.is_empty() => (Some(module), rest),
        _ => (None, name),
    }
}

fn is_html_type_name(name: &str) -> bool {
    let (_, simple) = split_type_name(name);
    simple == "Html"
}

fn is_html_response_type(ty: &TypeRef) -> bool {
    match &ty.kind {
        TypeRefKind::Simple(ident) => is_html_type_name(&ident.name),
        TypeRefKind::Refined { base, .. } => is_html_type_name(&base.name),
        TypeRefKind::Optional(inner) => is_html_response_type(inner),
        TypeRefKind::Result { ok, .. } => is_html_response_type(ok),
        TypeRefKind::Generic { base, args } => match base.name.as_str() {
            "Option" | "Result" => args.first().is_some_and(is_html_response_type),
            _ => false,
        },
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn label(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    fn json_label(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

fn parse_log_level(raw: &str) -> Option<LogLevel> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LogLevel::Trace),
        "debug" => Some(LogLevel::Debug),
        "info" => Some(LogLevel::Info),
        "warn" | "warning" => Some(LogLevel::Warn),
        "error" => Some(LogLevel::Error),
        _ => None,
    }
}

fn is_query_method(name: &str) -> bool {
    matches!(
        name,
        "select" | "where" | "order_by" | "limit" | "one" | "all" | "exec" | "sql" | "params"
    )
}

fn min_log_level() -> LogLevel {
    std::env::var("FUSE_LOG")
        .ok()
        .and_then(|raw| parse_log_level(&raw))
        .unwrap_or(LogLevel::Info)
}

fn builtin_error_defaults(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "std.Error.BadRequest" => Some(("bad_request", "bad request")),
        "std.Error.Unauthorized" => Some(("unauthorized", "unauthorized")),
        "std.Error.Forbidden" => Some(("forbidden", "forbidden")),
        "std.Error.NotFound" => Some(("not_found", "not found")),
        "std.Error.Conflict" => Some(("conflict", "conflict")),
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
    let Some(value) = value else {
        return out;
    };
    let value = value.unboxed();
    let Value::List(items) = value else {
        return out;
    };
    for item in items {
        let item = item.unboxed();
        let Value::Struct { fields, .. } = item else {
            continue;
        };
        let Some(Value::String(path)) = fields.get("path") else {
            continue;
        };
        let Some(Value::String(code)) = fields.get("code") else {
            continue;
        };
        let Some(Value::String(message)) = fields.get("message") else {
            continue;
        };
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
    Break,
    Continue,
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

    fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                return scope.get_mut(name);
            }
        }
        None
    }
}

pub struct Interpreter<'a> {
    env: Env,
    configs: HashMap<String, HashMap<String, Value>>,
    db: Option<Db>,
    functions: HashMap<String, &'a FnDecl>,
    apps: Vec<&'a AppDecl>,
    app_owner: HashMap<String, ModuleId>,
    services: HashMap<String, &'a ServiceDecl>,
    service_owner: HashMap<String, ModuleId>,
    types: HashMap<String, &'a TypeDecl>,
    config_decls: HashMap<String, &'a ConfigDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    module_maps: HashMap<ModuleId, ModuleMap>,
    function_owner: HashMap<String, ModuleId>,
    config_owner: HashMap<String, ModuleId>,
    current_module: ModuleId,
}

pub struct MigrationJob<'a> {
    pub id: String,
    pub module_id: ModuleId,
    pub decl: &'a MigrationDecl,
}

pub struct TestJob<'a> {
    pub name: String,
    pub module_id: ModuleId,
    pub decl: &'a TestDecl,
}

pub struct TestOutcome {
    pub name: String,
    pub ok: bool,
    pub message: Option<String>,
}

impl<'a> Interpreter<'a> {
    pub fn new(program: &'a Program) -> Self {
        Self::with_modules(program, ModuleMap::default())
    }

    pub fn with_modules(program: &'a Program, modules: ModuleMap) -> Self {
        let mut functions = HashMap::new();
        let mut function_owner = HashMap::new();
        let mut apps = Vec::new();
        let mut app_owner = HashMap::new();
        let mut services = HashMap::new();
        let mut service_owner = HashMap::new();
        let mut types = HashMap::new();
        let mut config_decls = HashMap::new();
        let mut config_owner = HashMap::new();
        let mut enums = HashMap::new();
        for item in &program.items {
            match item {
                Item::Fn(decl) => {
                    functions.insert(decl.name.name.clone(), decl);
                    function_owner.insert(decl.name.name.clone(), 0);
                }
                Item::App(app) => {
                    apps.push(app);
                    app_owner.insert(app.name.value.clone(), 0);
                }
                Item::Service(decl) => {
                    services.insert(decl.name.name.clone(), decl);
                    service_owner.insert(decl.name.name.clone(), 0);
                }
                Item::Type(decl) => {
                    types.insert(decl.name.name.clone(), decl);
                }
                Item::Config(decl) => {
                    config_decls.insert(decl.name.name.clone(), decl);
                    config_owner.insert(decl.name.name.clone(), 0);
                }
                Item::Enum(decl) => {
                    enums.insert(decl.name.name.clone(), decl);
                }
                _ => {}
            }
        }
        Self {
            env: Env::new(),
            configs: HashMap::new(),
            db: None,
            functions,
            apps,
            app_owner,
            services,
            service_owner,
            types,
            config_decls,
            enums,
            module_maps: HashMap::from([(0, modules)]),
            function_owner,
            config_owner,
            current_module: 0,
        }
    }

    pub fn with_registry(registry: &'a ModuleRegistry) -> Self {
        let mut functions = HashMap::new();
        let mut function_owner = HashMap::new();
        let mut apps = Vec::new();
        let mut app_owner = HashMap::new();
        let mut services = HashMap::new();
        let mut service_owner = HashMap::new();
        let mut types = HashMap::new();
        let mut config_decls = HashMap::new();
        let mut config_owner = HashMap::new();
        let mut enums = HashMap::new();
        let mut module_maps = HashMap::new();
        for (id, unit) in &registry.modules {
            module_maps.insert(*id, unit.modules.clone());
            for item in &unit.program.items {
                match item {
                    Item::Fn(decl) => {
                        functions.insert(decl.name.name.clone(), decl);
                        function_owner.insert(decl.name.name.clone(), *id);
                    }
                    Item::App(app) => {
                        apps.push(app);
                        app_owner.insert(app.name.value.clone(), *id);
                    }
                    Item::Service(decl) => {
                        services.insert(decl.name.name.clone(), decl);
                        service_owner.insert(decl.name.name.clone(), *id);
                    }
                    Item::Type(decl) => {
                        types.insert(decl.name.name.clone(), decl);
                    }
                    Item::Config(decl) => {
                        config_decls.insert(decl.name.name.clone(), decl);
                        config_owner.insert(decl.name.name.clone(), *id);
                    }
                    Item::Enum(decl) => {
                        enums.insert(decl.name.name.clone(), decl);
                    }
                    _ => {}
                }
            }
        }
        Self {
            env: Env::new(),
            configs: HashMap::new(),
            db: None,
            functions,
            apps,
            app_owner,
            services,
            service_owner,
            types,
            config_decls,
            enums,
            module_maps,
            function_owner,
            config_owner,
            current_module: registry.root,
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
        let prev_module = self.current_module;
        if let Some(owner) = self.app_owner.get(&app.name.value) {
            self.current_module = *owner;
        }
        let out = match self.eval_block(&app.body) {
            Ok(_) => Ok(()),
            Err(ExecError::Return(_)) => Ok(()),
            Err(err) => Err(self.render_exec_error(err)),
        };
        self.current_module = prev_module;
        out
    }

    pub fn run_migrations(&mut self, migrations: &[MigrationJob<'_>]) -> Result<(), String> {
        if let Err(err) = self.eval_configs() {
            return Err(self.render_exec_error(err));
        }
        {
            let db = match self.db_mut() {
                Ok(db) => db,
                Err(err) => return Err(self.render_exec_error(err)),
            };
            db.exec(
                "CREATE TABLE IF NOT EXISTS __fuse_migrations (id TEXT PRIMARY KEY, applied_at TEXT NOT NULL)",
            )?;
        }
        let applied_rows = {
            let db = match self.db_mut() {
                Ok(db) => db,
                Err(err) => return Err(self.render_exec_error(err)),
            };
            db.query("SELECT id FROM __fuse_migrations")?
        };
        let mut applied = HashSet::new();
        for row in applied_rows {
            if let Some(Value::String(id)) = row.get("id") {
                applied.insert(id.clone());
            }
        }
        for job in migrations {
            if applied.contains(&job.id) {
                continue;
            }
            {
                let db = match self.db_mut() {
                    Ok(db) => db,
                    Err(err) => return Err(self.render_exec_error(err)),
                };
                db.exec("BEGIN")?;
            }
            let prev_module = self.current_module;
            self.current_module = job.module_id;
            let result = self.eval_block(&job.decl.body);
            self.current_module = prev_module;
            match result {
                Ok(_) => {}
                Err(ExecError::Return(_)) => {
                    if let Ok(db) = self.db_mut() {
                        let _ = db.exec("ROLLBACK");
                    }
                    return Err("return not allowed in migration".to_string());
                }
                Err(err) => {
                    if let Ok(db) = self.db_mut() {
                        let _ = db.exec("ROLLBACK");
                    }
                    return Err(self.render_exec_error(err));
                }
            }
            {
                let db = match self.db_mut() {
                    Ok(db) => db,
                    Err(err) => return Err(self.render_exec_error(err)),
                };
                db.execute(
                    "INSERT INTO __fuse_migrations (id, applied_at) VALUES (?1, CURRENT_TIMESTAMP)",
                    (&job.id,),
                )?;
                db.exec("COMMIT")?;
            }
        }
        Ok(())
    }

    pub fn run_tests(&mut self, tests: &[TestJob<'_>]) -> Result<Vec<TestOutcome>, String> {
        if let Err(err) = self.eval_configs() {
            return Err(self.render_exec_error(err));
        }
        let mut out = Vec::new();
        for job in tests {
            let prev_module = self.current_module;
            self.current_module = job.module_id;
            let result = self.eval_block(&job.decl.body);
            self.current_module = prev_module;
            match result {
                Ok(_) => out.push(TestOutcome {
                    name: job.name.clone(),
                    ok: true,
                    message: None,
                }),
                Err(ExecError::Return(_)) => out.push(TestOutcome {
                    name: job.name.clone(),
                    ok: false,
                    message: Some("return not allowed in test".to_string()),
                }),
                Err(err) => out.push(TestOutcome {
                    name: job.name.clone(),
                    ok: false,
                    message: Some(self.render_exec_error(err)),
                }),
            }
        }
        Ok(out)
    }

    pub fn parse_cli_value(&mut self, ty: &TypeRef, raw: &str) -> Result<Value, String> {
        match self.parse_env_value(ty, raw) {
            Ok(value) => Ok(value),
            Err(err) => Err(self.render_exec_error(err)),
        }
    }

    pub(crate) fn prepare_call_with_named_args(
        &mut self,
        name: &str,
        args: &HashMap<String, Value>,
    ) -> Result<Vec<Value>, String> {
        let decl = match self.functions.get(name) {
            Some(decl) => *decl,
            None => return Err(format!("unknown function {name}")),
        };
        let prev_module = self.current_module;
        if let Some(owner) = self.function_owner.get(name) {
            self.current_module = *owner;
        }
        self.env.push();
        let mut ordered = Vec::with_capacity(decl.params.len());
        for param in &decl.params {
            let value = if let Some(value) = args.get(&param.name.name) {
                value.clone()
            } else if let Some(default) = &param.default {
                match self.eval_expr(default) {
                    Ok(value) => value,
                    Err(err) => {
                        self.env.pop();
                        self.current_module = prev_module;
                        return Err(self.render_exec_error(err));
                    }
                }
            } else if self.is_optional_type(&param.ty) {
                Value::Null
            } else {
                self.env.pop();
                self.current_module = prev_module;
                return Err(format!("missing argument {}", param.name.name));
            };
            if let Err(err) = self.validate_value(&value, &param.ty, &param.name.name) {
                self.env.pop();
                self.current_module = prev_module;
                return Err(self.render_exec_error(err));
            }
            self.env.insert(&param.name.name, value.clone());
            ordered.push(value);
        }
        self.env.pop();
        self.current_module = prev_module;
        Ok(ordered)
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
        let prev_module = self.current_module;
        if let Some(owner) = self.function_owner.get(name) {
            self.current_module = *owner;
        }
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
        let out = match result {
            Ok(value) => Ok(value),
            Err(ExecError::Return(value)) => Ok(value),
            Err(err) => Err(self.render_exec_error(err)),
        };
        self.current_module = prev_module;
        out
    }

    fn render_exec_error(&self, err: ExecError) -> String {
        match err {
            ExecError::Runtime(msg) => msg,
            ExecError::Error(value) => format_error_value(&value),
            ExecError::Return(value) => format!("unexpected return: {}", value.to_string_value()),
            ExecError::Break => "break outside of loop".to_string(),
            ExecError::Continue => "continue outside of loop".to_string(),
        }
    }

    fn eval_configs(&mut self) -> ExecResult<()> {
        let config_path =
            std::env::var("FUSE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        let file_values = rt_config::load_config_file(&config_path).map_err(ExecError::Runtime)?;
        let decls: Vec<&ConfigDecl> = self.config_decls.values().copied().collect();
        for decl in decls {
            let name = decl.name.name.clone();
            let prev_module = self.current_module;
            if let Some(owner) = self.config_owner.get(&name) {
                self.current_module = *owner;
            }
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
            self.current_module = prev_module;
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
                Err(ExecError::Break) => {
                    self.env.pop();
                    return Err(ExecError::Break);
                }
                Err(ExecError::Continue) => {
                    self.env.pop();
                    return Err(ExecError::Continue);
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
                _ => {
                    let value = self.eval_expr(expr)?;
                    self.assign_target(target, value)?;
                    Ok(Value::Unit)
                }
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
            StmtKind::For { pat, iter, block } => {
                let iter_value = self.eval_expr(iter)?;
                let iter_value = iter_value.unboxed();
                let items = match iter_value {
                    Value::List(items) => items,
                    Value::Map(items) => items.into_values().collect(),
                    other => {
                        return Err(ExecError::Runtime(format!(
                            "cannot iterate over {}",
                            self.value_type_name(&other)
                        )));
                    }
                };
                for item in items {
                    let mut bindings = HashMap::new();
                    if !self.match_pattern(&item, pat, &mut bindings)? {
                        return Err(ExecError::Runtime(
                            "for pattern did not match value".to_string(),
                        ));
                    }
                    self.env.push();
                    for (name, value) in bindings {
                        self.env.insert(&name, value);
                    }
                    let result = self.eval_block(block);
                    self.env.pop();
                    match result {
                        Ok(_) => {}
                        Err(ExecError::Break) => break,
                        Err(ExecError::Continue) => continue,
                        Err(other) => return Err(other),
                    }
                }
                Ok(Value::Unit)
            }
            StmtKind::While { cond, block } => {
                loop {
                    let cond_val = self.eval_expr(cond)?;
                    if !self.as_bool(&cond_val)? {
                        break;
                    }
                    match self.eval_block(block) {
                        Ok(_) => {}
                        Err(ExecError::Break) => break,
                        Err(ExecError::Continue) => continue,
                        Err(other) => return Err(other),
                    }
                }
                Ok(Value::Unit)
            }
            StmtKind::Break => Err(ExecError::Break),
            StmtKind::Continue => Err(ExecError::Continue),
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> ExecResult<Value> {
        match &expr.kind {
            ExprKind::Literal(lit) => Ok(self.value_from_literal(lit)),
            ExprKind::Ident(ident) => self.resolve_ident(&ident.name),
            ExprKind::Binary { op, left, right } => {
                let left_val = self.eval_expr(left)?.unboxed();
                let right_val = self.eval_expr(right)?.unboxed();
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
                    BinaryOp::Range => self.eval_range(left_val, right_val),
                }
            }
            ExprKind::Unary { op, expr } => {
                let value = self.eval_expr(expr)?.unboxed();
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
                if let ExprKind::Member { base, name } = &callee.kind {
                    let mut arg_vals = Vec::new();
                    for arg in args {
                        arg_vals.push(self.eval_expr(&arg.value)?);
                    }
                    if let Some(value) = self.eval_module_member(base, &name.name)? {
                        return self.eval_call(value, arg_vals);
                    }
                    if let ExprKind::Ident(ident) = &base.kind {
                        if self.enums.contains_key(&ident.name) {
                            let callee_val = self.eval_enum_member(&ident.name, &name.name)?;
                            return self.eval_call(callee_val, arg_vals);
                        }
                    }
                    let base_val = self.eval_expr(base)?.unboxed();
                    if is_query_method(&name.name) && matches!(base_val, Value::Query(_)) {
                        let mut query_args = Vec::with_capacity(args.len() + 1);
                        query_args.push(base_val);
                        query_args.extend(arg_vals);
                        return self.eval_builtin(&format!("query.{}", name.name), query_args);
                    }
                    let callee_val = self.eval_member(base_val, &name.name)?;
                    return self.eval_call(callee_val, arg_vals);
                }
                let callee_val = self.eval_expr(callee)?.unboxed();
                let mut arg_vals = Vec::new();
                for arg in args {
                    arg_vals.push(self.eval_expr(&arg.value)?);
                }
                self.eval_call(callee_val, arg_vals)
            }
            ExprKind::Member { base, name } => {
                if let Some(value) = self.eval_module_member(base, &name.name)? {
                    return Ok(value);
                }
                if let ExprKind::Ident(ident) = &base.kind {
                    if self.enums.contains_key(&ident.name) {
                        return self.eval_enum_member(&ident.name, &name.name);
                    }
                }
                let base_val = self.eval_expr(base)?.unboxed();
                self.eval_member(base_val, &name.name)
            }
            ExprKind::OptionalMember { base, name } => {
                let base_val = self.eval_expr(base)?.unboxed();
                if matches!(base_val, Value::Null) {
                    Ok(Value::Null)
                } else {
                    self.eval_member(base_val, &name.name)
                }
            }
            ExprKind::Index { base, index } => {
                let base_val = self.eval_expr(base)?.unboxed();
                let index_val = self.eval_expr(index)?.unboxed();
                self.eval_index(base_val, index_val)
            }
            ExprKind::OptionalIndex { base, index } => {
                let base_val = self.eval_expr(base)?.unboxed();
                if matches!(base_val, Value::Null) {
                    Ok(Value::Null)
                } else {
                    let index_val = self.eval_expr(index)?.unboxed();
                    self.eval_index(base_val, index_val)
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
                            )));
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
                if matches!(left_val.unboxed(), Value::Null) {
                    self.eval_expr(right)
                } else {
                    Ok(left_val)
                }
            }
            ExprKind::BangChain { expr, error } => self.eval_bang_chain(expr, error.as_deref()),
            ExprKind::Spawn { block } => {
                let result = self.eval_block(block);
                Ok(Value::Task(Task::new(result)))
            }
            ExprKind::Await { expr } => {
                let value = self.eval_expr(expr)?;
                match value {
                    Value::Task(task) => task.result(),
                    _ => Err(ExecError::Runtime("await expects a Task value".to_string())),
                }
            }
            ExprKind::Box { expr } => {
                let value = self.eval_expr(expr)?;
                match value {
                    Value::Boxed(_) => Ok(value),
                    other => Ok(Value::Boxed(Rc::new(RefCell::new(other)))),
                }
            }
        }
    }

    fn db_url(&self) -> ExecResult<String> {
        if let Ok(url) = std::env::var("FUSE_DB_URL") {
            return Ok(url);
        }
        if let Ok(url) = std::env::var("DATABASE_URL") {
            return Ok(url);
        }
        if let Some(Value::String(url)) = self
            .configs
            .get("App")
            .and_then(|config| config.get("dbUrl"))
        {
            return Ok(url.clone());
        }
        Err(ExecError::Runtime(
            "db url not configured (set FUSE_DB_URL or App.dbUrl)".to_string(),
        ))
    }

    fn db_mut(&mut self) -> ExecResult<&mut Db> {
        if self.db.is_none() {
            let url = self.db_url()?;
            let db = Db::open(&url).map_err(ExecError::Runtime)?;
            self.db = Some(db);
        }
        Ok(self.db.as_mut().expect("db initialized"))
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
            "print" | "env" | "serve" | "log" | "db" | "assert" | "task" | "json" | "html" => {
                Ok(Value::Builtin(name.to_string()))
            }
            _ => Err(ExecError::Runtime(format!("unknown identifier {name}"))),
        }
    }

    fn eval_call(&mut self, callee: Value, args: Vec<Value>) -> ExecResult<Value> {
        match callee.unboxed() {
            Value::Builtin(name) => self.eval_builtin(&name, args),
            Value::Function(name) => self.eval_function(&name, args),
            Value::EnumCtor { name, variant } => {
                let arity = self.enum_variant_arity(&name, &variant).ok_or_else(|| {
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
            _ => Err(ExecError::Runtime(
                "call target is not callable".to_string(),
            )),
        }
    }

    fn eval_builtin(&mut self, name: &str, args: Vec<Value>) -> ExecResult<Value> {
        let args: Vec<Value> = args.into_iter().map(|val| val.unboxed()).collect();
        match name {
            "print" => {
                let text = args.get(0).map(|v| v.to_string_value()).unwrap_or_default();
                println!("{text}");
                Ok(Value::Unit)
            }
            "log" => {
                let mut level = LogLevel::Info;
                let mut start_idx = 0usize;
                if args.len() >= 2 {
                    if let Some(Value::String(raw_level)) = args.get(0) {
                        if let Some(parsed) = parse_log_level(raw_level) {
                            level = parsed;
                            start_idx = 1;
                        }
                    }
                }
                if level >= min_log_level() {
                    let message = args
                        .get(start_idx)
                        .map(|val| val.to_string_value())
                        .unwrap_or_default();
                    let data_args = &args[start_idx.saturating_add(1)..];
                    if !data_args.is_empty() {
                        let data_json = if data_args.len() == 1 {
                            self.value_to_json(&data_args[0])
                        } else {
                            rt_json::JsonValue::Array(
                                data_args
                                    .iter()
                                    .map(|val| self.value_to_json(val))
                                    .collect(),
                            )
                        };
                        let mut obj = BTreeMap::new();
                        obj.insert(
                            "level".to_string(),
                            rt_json::JsonValue::String(level.json_label().to_string()),
                        );
                        obj.insert("message".to_string(), rt_json::JsonValue::String(message));
                        obj.insert("data".to_string(), data_json);
                        eprintln!("{}", rt_json::encode(&rt_json::JsonValue::Object(obj)));
                    } else {
                        let message = args[start_idx..]
                            .iter()
                            .map(|val| val.to_string_value())
                            .collect::<Vec<_>>()
                            .join(" ");
                        eprintln!("[{}] {}", level.label(), message);
                    }
                }
                Ok(Value::Unit)
            }
            "json.encode" => {
                let value = args
                    .get(0)
                    .cloned()
                    .ok_or_else(|| ExecError::Runtime("json.encode expects a value".to_string()))?;
                let json = self.value_to_json(&value);
                Ok(Value::String(rt_json::encode(&json)))
            }
            "json.decode" => {
                let text = match args.get(0) {
                    Some(Value::String(text)) => text.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "json.decode expects a string argument".to_string(),
                        ));
                    }
                };
                let json = rt_json::decode(&text)
                    .map_err(|msg| ExecError::Runtime(format!("invalid json: {msg}")))?;
                Ok(self.json_to_value(&json))
            }
            "html.text" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "html.text expects 1 argument".to_string(),
                    ));
                }
                let text = match args.get(0) {
                    Some(Value::String(text)) => text.clone(),
                    _ => {
                        return Err(ExecError::Runtime("html.text expects a String".to_string()));
                    }
                };
                Ok(Value::Html(HtmlNode::Text(text)))
            }
            "html.raw" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "html.raw expects 1 argument".to_string(),
                    ));
                }
                let text = match args.get(0) {
                    Some(Value::String(text)) => text.clone(),
                    _ => {
                        return Err(ExecError::Runtime("html.raw expects a String".to_string()));
                    }
                };
                Ok(Value::Html(HtmlNode::Raw(text)))
            }
            "html.node" => {
                if args.len() != 3 {
                    return Err(ExecError::Runtime(
                        "html.node expects 3 arguments".to_string(),
                    ));
                }
                let tag = match args.get(0) {
                    Some(Value::String(tag)) => tag.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "html.node expects a String tag".to_string(),
                        ));
                    }
                };
                let attrs = match args.get(1) {
                    Some(Value::Map(map)) => {
                        let mut attrs = HashMap::with_capacity(map.len());
                        for (key, value) in map {
                            let Value::String(text) = value else {
                                return Err(ExecError::Runtime(
                                    "html.node attrs must be Map<String, String>".to_string(),
                                ));
                            };
                            attrs.insert(key.clone(), text.clone());
                        }
                        attrs
                    }
                    _ => {
                        return Err(ExecError::Runtime(
                            "html.node expects attrs as Map<String, String>".to_string(),
                        ));
                    }
                };
                let children = match args.get(2) {
                    Some(Value::List(items)) => {
                        let mut children = Vec::with_capacity(items.len());
                        for item in items {
                            let Value::Html(node) = item else {
                                return Err(ExecError::Runtime(
                                    "html.node children must be List<Html>".to_string(),
                                ));
                            };
                            children.push(node.clone());
                        }
                        children
                    }
                    _ => {
                        return Err(ExecError::Runtime(
                            "html.node expects children as List<Html>".to_string(),
                        ));
                    }
                };
                Ok(Value::Html(HtmlNode::Element {
                    tag,
                    attrs,
                    children,
                }))
            }
            "html.render" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "html.render expects 1 argument".to_string(),
                    ));
                }
                let node = match args.get(0) {
                    Some(Value::Html(node)) => node,
                    _ => {
                        return Err(ExecError::Runtime(
                            "html.render expects an Html value".to_string(),
                        ));
                    }
                };
                Ok(Value::String(node.render_to_string()))
            }
            "db.exec" => {
                if args.len() > 2 {
                    return Err(ExecError::Runtime(
                        "db.exec expects 1 or 2 arguments".to_string(),
                    ));
                }
                let sql = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "db.exec expects a SQL string".to_string(),
                        ));
                    }
                };
                let params = if args.len() > 1 {
                    match args.get(1) {
                        Some(Value::List(items)) => items.clone(),
                        _ => {
                            return Err(ExecError::Runtime(
                                "db.exec params must be a list".to_string(),
                            ));
                        }
                    }
                } else {
                    Vec::new()
                };
                let db = self.db_mut()?;
                db.exec_params(&sql, &params).map_err(ExecError::Runtime)?;
                Ok(Value::Unit)
            }
            "db.query" => {
                if args.len() > 2 {
                    return Err(ExecError::Runtime(
                        "db.query expects 1 or 2 arguments".to_string(),
                    ));
                }
                let sql = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "db.query expects a SQL string".to_string(),
                        ));
                    }
                };
                let params = if args.len() > 1 {
                    match args.get(1) {
                        Some(Value::List(items)) => items.clone(),
                        _ => {
                            return Err(ExecError::Runtime(
                                "db.query params must be a list".to_string(),
                            ));
                        }
                    }
                } else {
                    Vec::new()
                };
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(ExecError::Runtime)?;
                let list = rows.into_iter().map(Value::Map).collect();
                Ok(Value::List(list))
            }
            "db.one" => {
                if args.len() > 2 {
                    return Err(ExecError::Runtime(
                        "db.one expects 1 or 2 arguments".to_string(),
                    ));
                }
                let sql = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "db.one expects a SQL string".to_string(),
                        ));
                    }
                };
                let params = if args.len() > 1 {
                    match args.get(1) {
                        Some(Value::List(items)) => items.clone(),
                        _ => {
                            return Err(ExecError::Runtime(
                                "db.one params must be a list".to_string(),
                            ));
                        }
                    }
                } else {
                    Vec::new()
                };
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(ExecError::Runtime)?;
                if let Some(row) = rows.into_iter().next() {
                    Ok(Value::Map(row))
                } else {
                    Ok(Value::Null)
                }
            }
            "db.from" => {
                let table = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "db.from expects a table name string".to_string(),
                        ));
                    }
                };
                let query = Query::new(table).map_err(ExecError::Runtime)?;
                Ok(Value::Query(query))
            }
            "query.select" => {
                if args.len() != 2 {
                    return Err(ExecError::Runtime(
                        "query.select expects 2 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.select expects a Query".to_string(),
                        ));
                    }
                };
                let columns = match args.get(1) {
                    Some(Value::List(items)) => {
                        let mut out = Vec::with_capacity(items.len());
                        for item in items {
                            match item {
                                Value::String(text) => out.push(text.clone()),
                                _ => {
                                    return Err(ExecError::Runtime(
                                        "query.select expects a list of strings".to_string(),
                                    ));
                                }
                            }
                        }
                        out
                    }
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.select expects a list of strings".to_string(),
                        ));
                    }
                };
                let next = query.select(columns).map_err(ExecError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.where" => {
                if args.len() != 4 {
                    return Err(ExecError::Runtime(
                        "query.where expects 4 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.where expects a Query".to_string(),
                        ));
                    }
                };
                let column = match args.get(1) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.where expects a column string".to_string(),
                        ));
                    }
                };
                let op = match args.get(2) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.where expects an operator string".to_string(),
                        ));
                    }
                };
                let value = args
                    .get(3)
                    .cloned()
                    .ok_or_else(|| ExecError::Runtime("query.where expects a value".to_string()))?;
                let next = query
                    .where_clause(column, op, value)
                    .map_err(ExecError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.order_by" => {
                if args.len() != 3 {
                    return Err(ExecError::Runtime(
                        "query.order_by expects 3 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.order_by expects a Query".to_string(),
                        ));
                    }
                };
                let column = match args.get(1) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.order_by expects a column string".to_string(),
                        ));
                    }
                };
                let dir = match args.get(2) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.order_by expects a direction string".to_string(),
                        ));
                    }
                };
                let next = query.order_by(column, dir).map_err(ExecError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.limit" => {
                if args.len() != 2 {
                    return Err(ExecError::Runtime(
                        "query.limit expects 2 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.limit expects a Query".to_string(),
                        ));
                    }
                };
                let limit = match args.get(1) {
                    Some(Value::Int(v)) => *v,
                    Some(Value::Float(v)) => *v as i64,
                    _ => return Err(ExecError::Runtime("query.limit expects an Int".to_string())),
                };
                let next = query.limit(limit).map_err(ExecError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.one" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "query.one expects 1 argument".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(ExecError::Runtime("query.one expects a Query".to_string())),
                };
                let (sql, params) = query.build_sql(Some(1)).map_err(ExecError::Runtime)?;
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(ExecError::Runtime)?;
                if let Some(row) = rows.into_iter().next() {
                    Ok(Value::Map(row))
                } else {
                    Ok(Value::Null)
                }
            }
            "query.all" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "query.all expects 1 argument".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(ExecError::Runtime("query.all expects a Query".to_string())),
                };
                let (sql, params) = query.build_sql(None).map_err(ExecError::Runtime)?;
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(ExecError::Runtime)?;
                let list = rows.into_iter().map(Value::Map).collect();
                Ok(Value::List(list))
            }
            "query.exec" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "query.exec expects 1 argument".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(ExecError::Runtime("query.exec expects a Query".to_string())),
                };
                let (sql, params) = query.build_sql(None).map_err(ExecError::Runtime)?;
                let db = self.db_mut()?;
                db.exec_params(&sql, &params).map_err(ExecError::Runtime)?;
                Ok(Value::Unit)
            }
            "query.sql" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "query.sql expects 1 argument".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(ExecError::Runtime("query.sql expects a Query".to_string())),
                };
                let sql = query.sql().map_err(ExecError::Runtime)?;
                Ok(Value::String(sql))
            }
            "query.params" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "query.params expects 1 argument".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "query.params expects a Query".to_string(),
                        ));
                    }
                };
                let params = query.params().map_err(ExecError::Runtime)?;
                Ok(Value::List(params))
            }
            "task.id" => match args.get(0) {
                Some(Value::Task(task)) => Ok(Value::String(format!("task-{}", task.id()))),
                _ => Err(ExecError::Runtime(
                    "task.id expects a Task argument".to_string(),
                )),
            },
            "task.done" => match args.get(0) {
                Some(Value::Task(task)) => Ok(Value::Bool(task.is_done())),
                _ => Err(ExecError::Runtime(
                    "task.done expects a Task argument".to_string(),
                )),
            },
            "task.cancel" => match args.get(0) {
                Some(Value::Task(task)) => Ok(Value::Bool(task.cancel())),
                _ => Err(ExecError::Runtime(
                    "task.cancel expects a Task argument".to_string(),
                )),
            },
            "assert" => {
                let cond = match args.get(0) {
                    Some(Value::Bool(value)) => *value,
                    _ => {
                        return Err(ExecError::Runtime(
                            "assert expects a Bool as the first argument".to_string(),
                        ));
                    }
                };
                if cond {
                    return Ok(Value::Unit);
                }
                let message = args
                    .get(1)
                    .map(|val| val.to_string_value())
                    .unwrap_or_else(|| "assertion failed".to_string());
                Err(ExecError::Runtime(format!("assert failed: {message}")))
            }
            "env" => {
                let key = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(ExecError::Runtime(
                            "env expects a string argument".to_string(),
                        ));
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
                        ));
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
        let prev_module = self.current_module;
        if let Some(owner) = self.function_owner.get(name) {
            self.current_module = *owner;
        }
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
        let out = match result {
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
            Err(ExecError::Break) => Err(ExecError::Runtime("break outside of loop".to_string())),
            Err(ExecError::Continue) => {
                Err(ExecError::Runtime("continue outside of loop".to_string()))
            }
        };
        self.current_module = prev_module;
        out
    }

    fn eval_serve(&mut self, port: i64) -> ExecResult<Value> {
        let service = self.select_service()?.clone();
        let prev_module = self.current_module;
        if let Some(owner) = self.service_owner.get(&service.name.name) {
            self.current_module = *owner;
        }
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
                    )));
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
        self.current_module = prev_module;
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
            _ => return Ok(self.http_response(405, self.internal_error_json("method not allowed"))),
        };
        let path = request
            .path
            .split('?')
            .next()
            .unwrap_or(&request.path)
            .to_string();
        if let Some(response) = self.try_static_response(request.method.as_str(), &path) {
            return Ok(response);
        }
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
        let html_response = is_html_response_type(&route.ret_type);
        match value {
            Value::ResultErr(err) => {
                let status = self.http_status_for_error_value(&err);
                let json = self.error_json_from_value(&err);
                Ok(self.http_response(status, json))
            }
            Value::ResultOk(ok) => {
                if html_response {
                    let body = self.render_html_value(ok.as_ref())?;
                    Ok(self.http_response_with_type(200, body, "text/html; charset=utf-8"))
                } else {
                    let json = self.value_to_json(&ok);
                    Ok(self.http_response(200, rt_json::encode(&json)))
                }
            }
            other => {
                if html_response {
                    let body = self.render_html_value(&other)?;
                    Ok(self.http_response_with_type(200, body, "text/html; charset=utf-8"))
                } else {
                    let json = self.value_to_json(&other);
                    Ok(self.http_response(200, rt_json::encode(&json)))
                }
            }
        }
    }

    fn try_static_response(&self, method: &str, path: &str) -> Option<String> {
        if method != "GET" {
            return None;
        }
        let static_dir = std::env::var("FUSE_STATIC_DIR").ok()?;
        let index = std::env::var("FUSE_STATIC_INDEX").unwrap_or_else(|_| "index.html".to_string());
        let rel_path = if path.is_empty() || path == "/" {
            index
        } else if path.ends_with('/') {
            format!("{}{}", path.trim_start_matches('/'), index)
        } else {
            path.trim_start_matches('/').to_string()
        };
        let rel = Path::new(&rel_path);
        if rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return None;
        }
        let full = Path::new(&static_dir).join(rel);
        if !full.is_file() {
            return None;
        }
        let body = fs::read_to_string(&full).ok()?;
        let content_type = match full.extension().and_then(|ext| ext.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("js") => "application/javascript; charset=utf-8",
            Some("json") => "application/json; charset=utf-8",
            _ => "text/plain; charset=utf-8",
        };
        Some(self.http_response_with_type(200, body, content_type))
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
        self.http_response_with_type(status, body, "application/json")
    }

    fn http_response_with_type(&self, status: u16, body: String, content_type: &str) -> String {
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
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
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
            ExecError::Break => {
                self.http_response(500, self.internal_error_json("break outside loop"))
            }
            ExecError::Continue => {
                self.http_response(500, self.internal_error_json("continue outside loop"))
            }
        }
    }

    fn http_status_for_error_value(&self, value: &Value) -> u16 {
        match value {
            Value::Struct { name, fields } => match name.as_str() {
                "std.Error.Validation" | "Validation" => 400,
                "std.Error.BadRequest" | "BadRequest" => 400,
                "std.Error.Unauthorized" | "Unauthorized" => 401,
                "std.Error.Forbidden" | "Forbidden" => 403,
                "std.Error.NotFound" | "NotFound" => 404,
                "std.Error.Conflict" | "Conflict" => 409,
                "std.Error" | "Error" => fields
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

    fn render_html_value(&self, value: &Value) -> ExecResult<String> {
        match value.unboxed() {
            Value::Html(node) => Ok(node.render_to_string()),
            other => Err(ExecError::Runtime(format!(
                "expected Html response, got {}",
                self.value_type_name(&other)
            ))),
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
        match base.unboxed() {
            Value::Builtin(name) if name == "db" => match field {
                "exec" | "query" | "one" | "from" => Ok(Value::Builtin(format!("db.{field}"))),
                _ => Err(ExecError::Runtime(format!("unknown db method {field}"))),
            },
            Value::Builtin(name) if name == "json" => match field {
                "encode" | "decode" => Ok(Value::Builtin(format!("json.{field}"))),
                _ => Err(ExecError::Runtime(format!("unknown json method {field}"))),
            },
            Value::Builtin(name) if name == "task" => match field {
                "id" | "done" | "cancel" => Ok(Value::Builtin(format!("task.{field}"))),
                _ => Err(ExecError::Runtime(format!("unknown task method {field}"))),
            },
            Value::Builtin(name) if name == "html" => match field {
                "text" | "raw" | "node" | "render" => Ok(Value::Builtin(format!("html.{field}"))),
                _ => Err(ExecError::Runtime(format!("unknown html method {field}"))),
            },
            Value::Config(name) => {
                let map = self
                    .configs
                    .get(&name)
                    .ok_or_else(|| ExecError::Runtime(format!("unknown config {name}")))?;
                map.get(field).cloned().ok_or_else(|| {
                    ExecError::Runtime(format!("unknown config field {name}.{field}"))
                })
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

    fn eval_index(&self, base: Value, index: Value) -> ExecResult<Value> {
        match base.unboxed() {
            Value::List(items) => {
                let idx = Self::list_index(&index)?;
                items
                    .get(idx)
                    .cloned()
                    .ok_or_else(|| ExecError::Runtime("index out of bounds".to_string()))
            }
            Value::Map(items) => {
                let key = Self::map_key(&index)?;
                Ok(items.get(&key).cloned().unwrap_or(Value::Null))
            }
            Value::Null => Err(ExecError::Runtime("null access".to_string())),
            _ => Err(ExecError::Runtime(
                "index access not supported on this value".to_string(),
            )),
        }
    }

    fn eval_module_member(&mut self, base: &Expr, field: &str) -> ExecResult<Option<Value>> {
        let module_map = self.module_maps.get(&self.current_module);
        if let ExprKind::Member {
            base: inner_base,
            name: inner_name,
        } = &base.kind
        {
            if let ExprKind::Ident(module_ident) = &inner_base.kind {
                if let Some(module) = module_map.and_then(|map| map.get(&module_ident.name)) {
                    let member = inner_name.name.as_str();
                    if module.exports.enums.contains(member) {
                        let value = self.eval_enum_member(member, field)?;
                        return Ok(Some(value));
                    }
                    if module.exports.configs.contains(member) {
                        let value = self.eval_member(Value::Config(member.to_string()), field)?;
                        return Ok(Some(value));
                    }
                    return Err(ExecError::Runtime(format!(
                        "unknown module member {}.{}",
                        module_ident.name, inner_name.name
                    )));
                }
            }
        }
        if let ExprKind::Ident(module_ident) = &base.kind {
            if let Some(module) = module_map.and_then(|map| map.get(&module_ident.name)) {
                if module.exports.functions.contains(field) {
                    return Ok(Some(Value::Function(field.to_string())));
                }
                if module.exports.configs.contains(field) {
                    return Ok(Some(Value::Config(field.to_string())));
                }
                if module.exports.enums.contains(field) || module.exports.types.contains(field) {
                    return Err(ExecError::Runtime(format!(
                        "{}.{} is a type, not a value",
                        module_ident.name, field
                    )));
                }
                return Err(ExecError::Runtime(format!(
                    "unknown module member {}.{}",
                    module_ident.name, field
                )));
            }
        }
        Ok(None)
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
            None => return Err(ExecError::Runtime(format!("unknown type {}", name.name))),
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
        let value = self.eval_expr(expr)?.unboxed();
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
            other => Ok(other),
        }
    }

    fn assign_target(&mut self, target: &Expr, value: Value) -> ExecResult<()> {
        match &target.kind {
            ExprKind::Ident(ident) => {
                if let Some(Value::Boxed(cell)) = self.env.get(&ident.name) {
                    *cell.borrow_mut() = value.unboxed();
                    return Ok(());
                }
                if !self.env.assign(&ident.name, value) {
                    return Err(ExecError::Runtime(format!(
                        "unknown variable {}",
                        ident.name
                    )));
                }
                Ok(())
            }
            ExprKind::Member { .. }
            | ExprKind::OptionalMember { .. }
            | ExprKind::Index { .. }
            | ExprKind::OptionalIndex { .. } => {
                let mut steps = Vec::new();
                let root = self.collect_assign_steps(target, &mut steps)?;
                let Some(root) = root else {
                    return Err(ExecError::Runtime("invalid assignment target".to_string()));
                };
                if steps.is_empty() {
                    return Err(ExecError::Runtime("invalid assignment target".to_string()));
                }
                let Some(root_value) = self.env.get_mut(&root) else {
                    return Err(ExecError::Runtime(format!("unknown variable {}", root)));
                };
                Self::assign_in_value(root_value, &steps, value)
            }
            _ => Err(ExecError::Runtime("invalid assignment target".to_string())),
        }
    }

    fn collect_assign_steps(
        &mut self,
        target: &Expr,
        out: &mut Vec<AssignStep>,
    ) -> ExecResult<Option<String>> {
        match &target.kind {
            ExprKind::Ident(ident) => Ok(Some(ident.name.clone())),
            ExprKind::Member { base, name } => {
                let root = self.collect_assign_steps(base, out)?;
                out.push(AssignStep::Field {
                    name: name.name.clone(),
                    optional: false,
                });
                Ok(root)
            }
            ExprKind::OptionalMember { base, name } => {
                let root = self.collect_assign_steps(base, out)?;
                out.push(AssignStep::Field {
                    name: name.name.clone(),
                    optional: true,
                });
                Ok(root)
            }
            ExprKind::Index { base, index } => {
                let root = self.collect_assign_steps(base, out)?;
                let key = self.eval_expr(index)?;
                out.push(AssignStep::Index {
                    key,
                    optional: false,
                });
                Ok(root)
            }
            ExprKind::OptionalIndex { base, index } => {
                let root = self.collect_assign_steps(base, out)?;
                let key = self.eval_expr(index)?;
                out.push(AssignStep::Index {
                    key,
                    optional: true,
                });
                Ok(root)
            }
            _ => Ok(None),
        }
    }

    fn assign_in_value(target: &mut Value, path: &[AssignStep], value: Value) -> ExecResult<()> {
        if path.is_empty() {
            *target = value;
            return Ok(());
        }
        match target {
            Value::Boxed(cell) => {
                let mut inner = cell.borrow_mut();
                return Self::assign_in_value(&mut inner, path, value);
            }
            Value::Null => {
                if path[0].is_optional() {
                    return Err(ExecError::Runtime(
                        "cannot assign through optional access".to_string(),
                    ));
                }
            }
            _ => {}
        }
        match &path[0] {
            AssignStep::Field { name, .. } => match target {
                Value::Struct { fields, .. } => {
                    if path.len() == 1 {
                        fields.insert(name.clone(), value);
                        Ok(())
                    } else {
                        let next = fields
                            .get_mut(name)
                            .ok_or_else(|| ExecError::Runtime(format!("unknown field {name}")))?;
                        Self::assign_in_value(next, &path[1..], value)
                    }
                }
                _ => Err(ExecError::Runtime(
                    "assignment target must be a struct field".to_string(),
                )),
            },
            AssignStep::Index { key, .. } => match target {
                Value::List(items) => {
                    let idx = Self::list_index(key)?;
                    if idx >= items.len() {
                        return Err(ExecError::Runtime("index out of bounds".to_string()));
                    }
                    if path.len() == 1 {
                        items[idx] = value;
                        Ok(())
                    } else {
                        let next = items
                            .get_mut(idx)
                            .ok_or_else(|| ExecError::Runtime("index out of bounds".to_string()))?;
                        Self::assign_in_value(next, &path[1..], value)
                    }
                }
                Value::Map(items) => {
                    let key = Self::map_key(key)?;
                    if path.len() == 1 {
                        items.insert(key, value);
                        Ok(())
                    } else {
                        let next = items
                            .get_mut(&key)
                            .ok_or_else(|| ExecError::Runtime("missing map entry".to_string()))?;
                        Self::assign_in_value(next, &path[1..], value)
                    }
                }
                _ => Err(ExecError::Runtime(
                    "assignment target must be an indexable value".to_string(),
                )),
            },
        }
    }

    fn list_index(value: &Value) -> ExecResult<usize> {
        match value.unboxed() {
            Value::Int(v) if v >= 0 => Ok(v as usize),
            Value::Int(_) => Err(ExecError::Runtime("index out of bounds".to_string())),
            _ => Err(ExecError::Runtime("list index must be Int".to_string())),
        }
    }

    fn map_key(value: &Value) -> ExecResult<String> {
        match value.unboxed() {
            Value::String(key) => Ok(key),
            _ => Err(ExecError::Runtime("map keys must be strings".to_string())),
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
        match value.unboxed() {
            Value::Bool(v) => Ok(v),
            _ => Err(ExecError::Runtime("condition must be a Bool".to_string())),
        }
    }

    fn eval_add(&self, left: Value, right: Value) -> ExecResult<Value> {
        let left = left.unboxed();
        let right = right.unboxed();
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
        let left = left.unboxed();
        let right = right.unboxed();
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

    fn eval_range(&self, left: Value, right: Value) -> ExecResult<Value> {
        match (left.unboxed(), right.unboxed()) {
            (Value::Int(start), Value::Int(end)) => {
                if start > end {
                    return Err(ExecError::Runtime("range start must be <= end".to_string()));
                }
                let items = (start..=end).map(Value::Int).collect();
                Ok(Value::List(items))
            }
            (Value::Float(start), Value::Float(end)) => self.eval_float_range(start, end),
            (Value::Int(start), Value::Float(end)) => self.eval_float_range(start as f64, end),
            (Value::Float(start), Value::Int(end)) => self.eval_float_range(start, end as f64),
            _ => Err(ExecError::Runtime(
                "range expects numeric bounds".to_string(),
            )),
        }
    }

    fn eval_float_range(&self, start: f64, end: f64) -> ExecResult<Value> {
        if !start.is_finite() || !end.is_finite() {
            return Err(ExecError::Runtime("invalid range bounds".to_string()));
        }
        if start > end {
            return Err(ExecError::Runtime("range start must be <= end".to_string()));
        }
        let mut items = Vec::new();
        let mut current = start;
        let epsilon = 1e-9;
        while current <= end + epsilon {
            items.push(Value::Float(current));
            current += 1.0;
        }
        Ok(Value::List(items))
    }

    fn eval_compare(&self, op: &BinaryOp, left: Value, right: Value) -> ExecResult<Value> {
        let left = left.unboxed();
        let right = right.unboxed();
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
                _ => {
                    return Err(ExecError::Runtime(
                        "unsupported string comparison".to_string(),
                    ));
                }
            },
            (Value::Bytes(a), Value::Bytes(b)) => match op {
                BinaryOp::Eq => a == b,
                BinaryOp::NotEq => a != b,
                _ => {
                    return Err(ExecError::Runtime(
                        "unsupported bytes comparison".to_string(),
                    ));
                }
            },
            _ => {
                return Err(ExecError::Runtime(
                    "unsupported comparison operands".to_string(),
                ));
            }
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
        let value = value.unboxed();
        match &pat.kind {
            PatternKind::Wildcard => Ok(true),
            PatternKind::Literal(lit) => Ok(self.literal_matches(&value, lit)),
            PatternKind::Ident(ident) => self.match_ident_pattern(&value, ident, bindings),
            PatternKind::EnumVariant { name, args } => {
                self.match_enum_variant_pattern(&value, &name.name, args, bindings)
            }
            PatternKind::Struct { name, fields } => {
                self.match_struct_pattern(&value, &name.name, fields, bindings)
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
        if let Value::Enum {
            variant, payload, ..
        } = value
        {
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
        let value = value.unboxed();
        match (&value, lit) {
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

    fn parse_env_value(&mut self, ty: &TypeRef, raw: &str) -> ExecResult<Value> {
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
            TypeRefKind::Simple(ident) => {
                let (_, simple_name) = split_type_name(&ident.name);
                match simple_name {
                    "Int" | "Float" | "Bool" | "String" | "Id" | "Email" | "Bytes" | "Html" => {
                        self.parse_simple_env(&ident.name, raw)
                    }
                    _ => {
                        let json = rt_json::decode(raw).map_err(|msg| {
                            ExecError::Runtime(format!("invalid JSON value: {msg}"))
                        })?;
                        self.decode_json_value(&json, ty, "$")
                    }
                }
            }
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
                "List" | "Map" => {
                    let json = rt_json::decode(raw)
                        .map_err(|msg| ExecError::Runtime(format!("invalid JSON value: {msg}")))?;
                    self.decode_json_value(&json, ty, "$")
                }
                _ => {
                    let json = rt_json::decode(raw)
                        .map_err(|msg| ExecError::Runtime(format!("invalid JSON value: {msg}")))?;
                    self.decode_json_value(&json, ty, "$")
                }
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
            "String" | "Id" | "Email" => Ok(Value::String(raw.to_string())),
            "Bytes" => {
                let bytes = rt_bytes::decode_base64(raw)
                    .map_err(|msg| ExecError::Runtime(format!("invalid Bytes (base64): {msg}")))?;
                Ok(Value::Bytes(bytes))
            }
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
        let value = value.unboxed();
        match &ty.kind {
            TypeRefKind::Optional(inner) => {
                if matches!(value, Value::Null) {
                    Ok(())
                } else {
                    self.validate_value(&value, inner, path)
                }
            }
            TypeRefKind::Result { ok, err } => match value {
                Value::ResultOk(inner) => self.validate_value(&inner, ok, path),
                Value::ResultErr(inner) => {
                    if let Some(err_ty) = err {
                        self.validate_value(&inner, err_ty, path)
                    } else {
                        Ok(())
                    }
                }
                _ => Err(ExecError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Result, got {}", self.value_type_name(&value)),
                ))),
            },
            TypeRefKind::Refined { base, args } => {
                self.validate_simple(&value, &base.name, path)?;
                self.check_refined(&value, &base.name, args, path)
            }
            TypeRefKind::Simple(ident) => self.validate_simple(&value, &ident.name, path),
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
                        self.validate_value(&value, &args[0], path)
                    }
                }
                "Result" => {
                    if args.len() != 2 {
                        return Err(ExecError::Runtime(
                            "Result expects 2 type arguments".to_string(),
                        ));
                    }
                    match value {
                        Value::ResultOk(inner) => self.validate_value(&inner, &args[0], path),
                        Value::ResultErr(inner) => self.validate_value(&inner, &args[1], path),
                        _ => Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Result, got {}", self.value_type_name(&value)),
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
                            format!("expected List, got {}", self.value_type_name(&value)),
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
                            format!("expected Map, got {}", self.value_type_name(&value)),
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
        let value = value.unboxed();
        let type_name = self.value_type_name(&value);
        let (module, simple_name) = split_type_name(name);
        if module.is_none() {
            match simple_name {
                "Int" => {
                    if matches!(value, Value::Int(_)) {
                        return Ok(());
                    }
                    return Err(ExecError::Runtime(format!("expected Int, got {type_name}")));
                }
                "Float" => {
                    if matches!(value, Value::Float(_)) {
                        return Ok(());
                    }
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Float, got {type_name}"),
                    )));
                }
                "Bool" => {
                    if matches!(value, Value::Bool(_)) {
                        return Ok(());
                    }
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bool, got {type_name}"),
                    )));
                }
                "String" => {
                    if matches!(value, Value::String(_)) {
                        return Ok(());
                    }
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected String, got {type_name}"),
                    )));
                }
                "Id" => match value {
                    Value::String(s) if !s.is_empty() => return Ok(()),
                    Value::String(_) => {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "expected non-empty Id".to_string(),
                        )));
                    }
                    _ => {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Id, got {type_name}"),
                        )));
                    }
                },
                "Email" => match value {
                    Value::String(s) if rt_validate::is_email(&s) => return Ok(()),
                    Value::String(_) => {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "invalid email address".to_string(),
                        )));
                    }
                    _ => {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Email, got {type_name}"),
                        )));
                    }
                },
                "Bytes" => match value {
                    Value::Bytes(_) => {
                        return Ok(());
                    }
                    _ => {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Bytes, got {type_name}"),
                        )));
                    }
                },
                "Html" => match value {
                    Value::Html(_) => {
                        return Ok(());
                    }
                    _ => {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Html, got {type_name}"),
                        )));
                    }
                },
                _ => {}
            }
        }
        match value {
            Value::Struct {
                name: struct_name, ..
            } if struct_name == simple_name => Ok(()),
            Value::Enum {
                name: enum_name, ..
            } if enum_name == simple_name => Ok(()),
            _ => Err(ExecError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("expected {name}, got {type_name}"),
            ))),
        }
    }

    fn value_type_name(&self, value: &Value) -> String {
        match value.unboxed() {
            Value::Unit => "Unit".to_string(),
            Value::Int(_) => "Int".to_string(),
            Value::Float(_) => "Float".to_string(),
            Value::Bool(_) => "Bool".to_string(),
            Value::String(_) => "String".to_string(),
            Value::Bytes(_) => "Bytes".to_string(),
            Value::Html(_) => "Html".to_string(),
            Value::Null => "Null".to_string(),
            Value::List(_) => "List".to_string(),
            Value::Map(_) => "Map".to_string(),
            Value::Query(_) => "Query".to_string(),
            Value::Task(_) => "Task".to_string(),
            Value::Iterator(_) => "Iterator".to_string(),
            Value::Struct { name, .. } => name.clone(),
            Value::Enum { name, .. } => name.clone(),
            Value::EnumCtor { name, .. } => name.clone(),
            Value::ResultOk(_) | Value::ResultErr(_) => "Result".to_string(),
            Value::Config(_) => "Config".to_string(),
            Value::Function(_) => "Function".to_string(),
            Value::Builtin(_) => "Builtin".to_string(),
            Value::Boxed(_) => "Box".to_string(),
        }
    }

    fn value_to_json(&self, value: &Value) -> rt_json::JsonValue {
        match value.unboxed() {
            Value::Unit => rt_json::JsonValue::Null,
            Value::Int(v) => rt_json::JsonValue::Number(v as f64),
            Value::Float(v) => rt_json::JsonValue::Number(v),
            Value::Bool(v) => rt_json::JsonValue::Bool(v),
            Value::String(v) => rt_json::JsonValue::String(v.clone()),
            Value::Bytes(v) => rt_json::JsonValue::String(rt_bytes::encode_base64(&v)),
            Value::Html(node) => rt_json::JsonValue::String(node.render_to_string()),
            Value::Null => rt_json::JsonValue::Null,
            Value::List(items) => {
                rt_json::JsonValue::Array(items.iter().map(|v| self.value_to_json(v)).collect())
            }
            Value::Map(items) => {
                let mut out = BTreeMap::new();
                for (key, value) in items {
                    out.insert(key.clone(), self.value_to_json(&value));
                }
                rt_json::JsonValue::Object(out)
            }
            Value::Boxed(_) => rt_json::JsonValue::String("<box>".to_string()),
            Value::Query(_) => rt_json::JsonValue::String("<query>".to_string()),
            Value::Task(_) => rt_json::JsonValue::String("<task>".to_string()),
            Value::Iterator(_) => rt_json::JsonValue::String("<iterator>".to_string()),
            Value::Struct { fields, .. } => {
                let mut out = BTreeMap::new();
                for (key, value) in fields {
                    out.insert(key.clone(), self.value_to_json(&value));
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
            Value::ResultOk(value) => self.value_to_json(value.as_ref()),
            Value::ResultErr(value) => self.value_to_json(value.as_ref()),
            Value::Config(name) => rt_json::JsonValue::String(name.clone()),
            Value::Function(name) => rt_json::JsonValue::String(name.clone()),
            Value::Builtin(name) => rt_json::JsonValue::String(name.clone()),
            Value::EnumCtor { name, variant } => {
                rt_json::JsonValue::String(format!("{name}.{variant}"))
            }
        }
    }

    fn json_to_value(&self, json: &rt_json::JsonValue) -> Value {
        match json {
            rt_json::JsonValue::Null => Value::Null,
            rt_json::JsonValue::Bool(v) => Value::Bool(*v),
            rt_json::JsonValue::Number(n) => {
                if n.fract() == 0.0 {
                    Value::Int(*n as i64)
                } else {
                    Value::Float(*n)
                }
            }
            rt_json::JsonValue::String(v) => Value::String(v.clone()),
            rt_json::JsonValue::Array(items) => {
                Value::List(items.iter().map(|item| self.json_to_value(item)).collect())
            }
            rt_json::JsonValue::Object(items) => {
                let mut out = HashMap::new();
                for (key, value) in items {
                    out.insert(key.clone(), self.json_to_value(value));
                }
                Value::Map(out)
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
                let (module, simple_name) = split_type_name(&ident.name);
                if module.is_none() {
                    if let Some(value) = self.decode_simple_json(json, simple_name, path)? {
                        value
                    } else if self.types.contains_key(simple_name) {
                        self.decode_struct_json(json, simple_name, path)?
                    } else if self.enums.contains_key(simple_name) {
                        self.decode_enum_json(json, simple_name, path)?
                    } else {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("unknown type {}", ident.name),
                        )));
                    }
                } else if self.types.contains_key(simple_name) {
                    self.decode_struct_json(json, simple_name, path)?
                } else if self.enums.contains_key(simple_name) {
                    self.decode_enum_json(json, simple_name, path)?
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
                )));
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
                    )));
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
                    )));
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
                    )));
                }
            },
            "Float" => match json {
                rt_json::JsonValue::Number(n) => Value::Float(*n),
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Float",
                    )));
                }
            },
            "Bool" => match json {
                rt_json::JsonValue::Bool(v) => Value::Bool(*v),
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Bool",
                    )));
                }
            },
            "String" | "Id" | "Email" => match json {
                rt_json::JsonValue::String(v) => Value::String(v.clone()),
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected String",
                    )));
                }
            },
            "Bytes" => match json {
                rt_json::JsonValue::String(v) => {
                    let bytes = rt_bytes::decode_base64(&v).map_err(|msg| {
                        ExecError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            format!("invalid Bytes (base64): {msg}"),
                        ))
                    })?;
                    Value::Bytes(bytes)
                }
                _ => {
                    return Err(ExecError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected String",
                    )));
                }
            },
            "Html" => {
                return Err(ExecError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    "expected Html",
                )));
            }
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
                vec![self.decode_json_value(data, &variant.payload[0], &format!("{path}.data"))?]
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
                    out.push(self.decode_json_value(item, ty, &format!("{path}.data[{idx}]"))?);
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

    fn check_refined(
        &self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> ExecResult<()> {
        let value = value.unboxed();
        match base {
            "String" => {
                let (min, max) = self.parse_length_range(args)?;
                let len = match value {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected String"
                        )));
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
                    Value::Int(v) => v,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Int"
                        )));
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
                    Value::Float(v) => v,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Float"
                        )));
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
            name: "std.Error.Validation".to_string(),
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
            name: "std.Error".to_string(),
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
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
