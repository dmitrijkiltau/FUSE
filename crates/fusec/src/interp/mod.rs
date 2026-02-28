use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

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
use crate::callbind::{
    CallArgSpec, CallBindError, ParamBinding, ParamSpec, bind_call_args, bind_positional_args,
};
use crate::db::{DEFAULT_DB_POOL_SIZE, Db, Query, parse_db_pool_size, parse_db_pool_size_value};
use crate::frontend::html_shorthand::{CanonicalizationPhase, validate_named_args_for_phase};
use crate::frontend::html_tag_builtin::should_use_html_tag_builtin;
use crate::html_tags::{self, HtmlTagKind};
use crate::loader::{ModuleId, ModuleLink, ModuleMap, ModuleRegistry};
use crate::observability;
use crate::refinement::{
    NumberLiteral, RefinementConstraint, base_is_string_like, parse_constraints,
};

#[derive(Clone, Debug)]
pub struct FunctionRef {
    pub(crate) module_id: ModuleId,
    pub(crate) name: String,
}

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
    Boxed(Arc<Mutex<Value>>),
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
    Function(FunctionRef),
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
    state: Arc<Mutex<TaskState>>,
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

#[derive(Debug)]
struct TaskState {
    id: u64,
    result: Option<TaskResult>,
    rx: Option<mpsc::Receiver<TaskResult>>,
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
        Task {
            state: Arc::new(Mutex::new(TaskState {
                id: NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed),
                result: Some(result),
                rx: None,
            })),
        }
    }

    pub(crate) fn spawn_async<F>(job: F) -> Self
    where
        F: FnOnce() -> TaskResult + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<TaskResult>();
        crate::task_pool::submit(move || {
            let _ = tx.send(job());
        });
        Task {
            state: Arc::new(Mutex::new(TaskState {
                id: NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed),
                result: None,
                rx: Some(rx),
            })),
        }
    }

    pub(crate) fn from_exec_result(result: ExecResult<Value>) -> TaskResult {
        match result {
            Ok(value) | Err(ExecError::Return(value)) => TaskResult::Ok(value),
            Err(ExecError::Error(value)) => TaskResult::Error(value),
            Err(ExecError::Runtime(message)) => TaskResult::Runtime(message),
            Err(ExecError::Break) => TaskResult::Runtime("break outside of loop".to_string()),
            Err(ExecError::Continue) => TaskResult::Runtime("continue outside of loop".to_string()),
        }
    }

    fn poll(state: &mut TaskState) {
        if state.result.is_some() {
            return;
        }
        let Some(rx) = state.rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                state.result = Some(result);
                state.rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                state.result = Some(TaskResult::Runtime(
                    "task execution failed (worker disconnected)".to_string(),
                ));
                state.rx = None;
            }
        }
    }

    pub(crate) fn result_raw(&self) -> TaskResult {
        {
            let mut state = self.state.lock().expect("task state lock");
            Self::poll(&mut state);
            if let Some(result) = state.result.clone() {
                return result;
            }
        }

        let rx = {
            let mut state = self.state.lock().expect("task state lock");
            match state.rx.take() {
                Some(rx) => rx,
                None => {
                    return state.result.clone().unwrap_or_else(|| {
                        TaskResult::Runtime("task state is missing result".to_string())
                    });
                }
            }
        };

        let result = rx.recv().unwrap_or_else(|_| {
            TaskResult::Runtime("task execution failed (worker disconnected)".to_string())
        });
        let mut state = self.state.lock().expect("task state lock");
        state.result = Some(result.clone());
        result
    }

    pub fn id(&self) -> u64 {
        self.state.lock().expect("task state lock").id
    }

    pub fn is_done(&self) -> bool {
        let mut state = self.state.lock().expect("task state lock");
        Self::poll(&mut state);
        state.result.is_some()
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
            Value::Function(func) => format!("<fn {}::{}>", func.module_id, func.name),
            Value::Builtin(name) => format!("<builtin {name}>"),
        }
    }

    pub(crate) fn unboxed(&self) -> Value {
        match self {
            Value::Boxed(cell) => cell.lock().expect("box lock").unboxed(),
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
    crate::runtime_types::split_type_name(name)
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

fn force_html_input_tag_call(name: &str, args: &[crate::ast::CallArg]) -> bool {
    if name != "input" {
        return false;
    }
    args.iter()
        .any(|arg| arg.name.is_some() || arg.is_block_sugar)
        || matches!(
            args.first().map(|arg| &arg.value.kind),
            Some(crate::ast::ExprKind::MapLit(_))
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
pub(crate) enum ExecError {
    Runtime(String),
    Return(Value),
    Error(Value),
    Break,
    Continue,
}

type ExecResult<T> = Result<T, ExecError>;

#[derive(Clone, Default)]
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

pub struct Interpreter {
    env: Env,
    configs: HashMap<String, HashMap<String, Value>>,
    db: Option<Db>,
    regex_cache: HashMap<String, regex::Regex>,
    functions: HashMap<ModuleId, HashMap<String, FnDecl>>,
    apps: Vec<AppDecl>,
    app_owner: HashMap<String, ModuleId>,
    services: HashMap<String, ServiceDecl>,
    service_owner: HashMap<String, ModuleId>,
    types: HashMap<String, TypeDecl>,
    config_decls: HashMap<String, ConfigDecl>,
    enums: HashMap<String, EnumDecl>,
    module_maps: HashMap<ModuleId, ModuleMap>,
    module_import_items: HashMap<ModuleId, HashMap<String, ModuleLink>>,
    config_owner: HashMap<String, ModuleId>,
    current_module: ModuleId,
    current_http_request: Option<HttpRequestContext>,
    current_http_response: Option<HttpResponseMeta>,
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

impl crate::runtime_types::RuntimeTypeHost for Interpreter {
    type Error = ExecError;

    fn runtime_error(&self, message: String) -> Self::Error {
        ExecError::Runtime(message)
    }

    fn validation_error(&self, path: &str, code: &str, message: String) -> Self::Error {
        ExecError::Error(crate::runtime_types::validation_error_value(
            path, code, message,
        ))
    }

    fn has_struct_type(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }

    fn has_enum_type(&self, name: &str) -> bool {
        self.enums.contains_key(name)
    }

    fn decode_struct_type_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> Result<Value, Self::Error> {
        Interpreter::decode_struct_json(self, json, name, path)
    }

    fn decode_enum_type_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> Result<Value, Self::Error> {
        Interpreter::decode_enum_json(self, json, name, path)
    }

    fn check_refined_value(
        &mut self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> Result<(), Self::Error> {
        Interpreter::check_refined(self, value, base, args, path)
    }
}

impl Interpreter {
    pub fn new(program: &Program) -> Self {
        Self::with_modules(program, ModuleMap::default())
    }

    pub fn with_modules(program: &Program, modules: ModuleMap) -> Self {
        let mut functions: HashMap<ModuleId, HashMap<String, FnDecl>> = HashMap::new();
        let mut apps = Vec::new();
        let mut app_owner = HashMap::new();
        let mut services = HashMap::new();
        let mut service_owner = HashMap::new();
        let mut types = HashMap::new();
        let mut config_decls = HashMap::new();
        let mut config_owner = HashMap::new();
        let mut enums = HashMap::new();
        let mut module_import_items: HashMap<ModuleId, HashMap<String, ModuleLink>> =
            HashMap::new();
        functions.insert(0, HashMap::new());
        module_import_items.insert(0, HashMap::new());
        for item in &program.items {
            match item {
                Item::Fn(decl) => {
                    functions
                        .entry(0)
                        .or_default()
                        .insert(decl.name.name.clone(), decl.clone());
                }
                Item::App(app) => {
                    apps.push(app.clone());
                    app_owner.insert(app.name.value.clone(), 0);
                }
                Item::Service(decl) => {
                    services.insert(decl.name.name.clone(), decl.clone());
                    service_owner.insert(decl.name.name.clone(), 0);
                }
                Item::Type(decl) => {
                    types.insert(decl.name.name.clone(), decl.clone());
                }
                Item::Config(decl) => {
                    config_decls.insert(decl.name.name.clone(), decl.clone());
                    config_owner.insert(decl.name.name.clone(), 0);
                }
                Item::Enum(decl) => {
                    enums.insert(decl.name.name.clone(), decl.clone());
                }
                _ => {}
            }
        }
        Self {
            env: Env::new(),
            configs: HashMap::new(),
            db: None,
            regex_cache: HashMap::new(),
            functions,
            apps,
            app_owner,
            services,
            service_owner,
            types,
            config_decls,
            enums,
            module_maps: HashMap::from([(0, modules)]),
            module_import_items,
            config_owner,
            current_module: 0,
            current_http_request: None,
            current_http_response: None,
        }
    }

    pub fn with_registry(registry: &ModuleRegistry) -> Self {
        let mut functions: HashMap<ModuleId, HashMap<String, FnDecl>> = HashMap::new();
        let mut apps = Vec::new();
        let mut app_owner = HashMap::new();
        let mut services = HashMap::new();
        let mut service_owner = HashMap::new();
        let mut types = HashMap::new();
        let mut config_decls = HashMap::new();
        let mut config_owner = HashMap::new();
        let mut enums = HashMap::new();
        let mut module_maps = HashMap::new();
        let mut module_import_items: HashMap<ModuleId, HashMap<String, ModuleLink>> =
            HashMap::new();
        for (id, unit) in &registry.modules {
            module_maps.insert(*id, unit.modules.clone());
            module_import_items.insert(*id, unit.import_items.clone());
            for item in &unit.program.items {
                match item {
                    Item::Fn(decl) => {
                        functions
                            .entry(*id)
                            .or_default()
                            .insert(decl.name.name.clone(), decl.clone());
                    }
                    Item::App(app) => {
                        apps.push(app.clone());
                        app_owner.insert(app.name.value.clone(), *id);
                    }
                    Item::Service(decl) => {
                        services.insert(decl.name.name.clone(), decl.clone());
                        service_owner.insert(decl.name.name.clone(), *id);
                    }
                    Item::Type(decl) => {
                        types.insert(decl.name.name.clone(), decl.clone());
                    }
                    Item::Config(decl) => {
                        config_decls.insert(decl.name.name.clone(), decl.clone());
                        config_owner.insert(decl.name.name.clone(), *id);
                    }
                    Item::Enum(decl) => {
                        enums.insert(decl.name.name.clone(), decl.clone());
                    }
                    _ => {}
                }
            }
        }
        Self {
            env: Env::new(),
            configs: HashMap::new(),
            db: None,
            regex_cache: HashMap::new(),
            functions,
            apps,
            app_owner,
            services,
            service_owner,
            types,
            config_decls,
            enums,
            module_maps,
            module_import_items,
            config_owner,
            current_module: registry.root,
            current_http_request: None,
            current_http_response: None,
        }
    }

    fn spawn_worker(&self) -> Self {
        Self {
            env: self.env.clone(),
            configs: self.configs.clone(),
            db: None,
            regex_cache: HashMap::new(),
            functions: self.functions.clone(),
            apps: self.apps.clone(),
            app_owner: self.app_owner.clone(),
            services: self.services.clone(),
            service_owner: self.service_owner.clone(),
            types: self.types.clone(),
            config_decls: self.config_decls.clone(),
            enums: self.enums.clone(),
            module_maps: self.module_maps.clone(),
            module_import_items: self.module_import_items.clone(),
            config_owner: self.config_owner.clone(),
            current_module: self.current_module,
            current_http_request: None,
            current_http_response: None,
        }
    }

    fn has_function_in_module(&self, module_id: ModuleId, name: &str) -> bool {
        self.functions
            .get(&module_id)
            .is_some_and(|items| items.contains_key(name))
    }

    fn function_decl(&self, func: &FunctionRef) -> Option<&FnDecl> {
        self.functions
            .get(&func.module_id)
            .and_then(|items| items.get(&func.name))
    }

    fn resolve_function_ref(&self, module_id: ModuleId, name: &str) -> Option<FunctionRef> {
        if self.has_function_in_module(module_id, name) {
            return Some(FunctionRef {
                module_id,
                name: name.to_string(),
            });
        }
        let import_items = self.module_import_items.get(&module_id)?;
        let link = import_items.get(name)?;
        if self.has_function_in_module(link.id, name) {
            return Some(FunctionRef {
                module_id: link.id,
                name: name.to_string(),
            });
        }
        None
    }

    pub fn run_app(&mut self, name: Option<&str>) -> Result<(), String> {
        if let Err(err) = self.eval_configs() {
            return Err(self.render_exec_error(err));
        }
        let app = if let Some(name) = name {
            self.apps
                .iter()
                .find(|app| app.name.value == name)
                .cloned()
                .ok_or_else(|| format!("app not found: {name}"))?
        } else {
            self.apps
                .first()
                .cloned()
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
                db.begin_transaction()?;
            }
            let prev_module = self.current_module;
            self.current_module = job.module_id;
            let result = self.eval_block(&job.decl.body);
            self.current_module = prev_module;
            match result {
                Ok(_) => {}
                Err(ExecError::Return(_)) => {
                    if let Ok(db) = self.db_mut() {
                        let _ = db.rollback_transaction();
                    }
                    return Err("return not allowed in migration".to_string());
                }
                Err(err) => {
                    if let Ok(db) = self.db_mut() {
                        let _ = db.rollback_transaction();
                    }
                    return Err(self.render_exec_error(err));
                }
            }
            {
                let db = match self.db_mut() {
                    Ok(db) => db,
                    Err(err) => return Err(self.render_exec_error(err)),
                };
                if let Err(err) = db.execute(
                    "INSERT INTO __fuse_migrations (id, applied_at) VALUES (?1, CURRENT_TIMESTAMP)",
                    (&job.id,),
                ) {
                    let _ = db.rollback_transaction();
                    return Err(err);
                }
                if let Err(err) = db.commit_transaction() {
                    let _ = db.rollback_transaction();
                    return Err(err);
                }
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

    pub fn prepare_call_with_named_args(
        &mut self,
        name: &str,
        args: &HashMap<String, Value>,
    ) -> Result<Vec<Value>, String> {
        let func_ref = match self.resolve_function_ref(self.current_module, name) {
            Some(func_ref) => func_ref,
            None => {
                return Err(format!(
                    "unknown function {name} in current module/import scope"
                ));
            }
        };
        let decl = match self.function_decl(&func_ref) {
            Some(decl) => decl.clone(),
            None => {
                return Err(format!(
                    "unknown function {}::{}",
                    func_ref.module_id, func_ref.name
                ));
            }
        };
        let prev_module = self.current_module;
        self.current_module = func_ref.module_id;
        let out = (|| -> Result<Vec<Value>, String> {
            self.env.push();
            let result = (|| -> Result<Vec<Value>, String> {
                let param_specs: Vec<ParamSpec<'_>> = decl
                    .params
                    .iter()
                    .map(|param| ParamSpec {
                        name: &param.name.name,
                        has_default: param.default.is_some() || self.is_optional_type(&param.ty),
                    })
                    .collect();
                let arg_specs: Vec<CallArgSpec<'_>> = args
                    .keys()
                    .map(|name| CallArgSpec {
                        name: Some(name.as_str()),
                    })
                    .collect();
                let (plan, bind_errors) = bind_call_args(&param_specs, &arg_specs);
                if let Some(err) = bind_errors.first() {
                    return Err(self.format_named_call_bind_error(err));
                }

                let mut ordered = Vec::with_capacity(decl.params.len());
                for (idx, param) in decl.params.iter().enumerate() {
                    let value = match plan.param_bindings.get(idx) {
                        Some(ParamBinding::Arg(_)) => args
                            .get(&param.name.name)
                            .cloned()
                            .expect("bound named argument should exist"),
                        Some(ParamBinding::Default) => {
                            if let Some(default) = &param.default {
                                self.eval_expr(default)
                                    .map_err(|err| self.render_exec_error(err))?
                            } else if self.is_optional_type(&param.ty) {
                                Value::Null
                            } else {
                                return Err(format!("missing argument {}", param.name.name));
                            }
                        }
                        Some(ParamBinding::MissingRequired) | None => {
                            return Err(format!("missing argument {}", param.name.name));
                        }
                    };
                    self.validate_value(&value, &param.ty, &param.name.name)
                        .map_err(|err| self.render_exec_error(err))?;
                    self.env.insert(&param.name.name, value.clone());
                    ordered.push(value);
                }
                Ok(ordered)
            })();
            self.env.pop();
            result
        })();
        self.current_module = prev_module;
        out
    }

    pub fn call_function_with_named_args(
        &mut self,
        name: &str,
        args: &HashMap<String, Value>,
    ) -> Result<Value, String> {
        let func_ref = match self.resolve_function_ref(self.current_module, name) {
            Some(func_ref) => func_ref,
            None => {
                return Err(format!(
                    "unknown function {name} in current module/import scope"
                ));
            }
        };
        let decl = match self.function_decl(&func_ref) {
            Some(decl) => decl.clone(),
            None => {
                return Err(format!(
                    "unknown function {}::{}",
                    func_ref.module_id, func_ref.name
                ));
            }
        };
        let prev_module = self.current_module;
        self.current_module = func_ref.module_id;
        let out = (|| -> Result<Value, String> {
            self.env.push();
            let result = (|| -> Result<Value, String> {
                let param_specs: Vec<ParamSpec<'_>> = decl
                    .params
                    .iter()
                    .map(|param| ParamSpec {
                        name: &param.name.name,
                        has_default: param.default.is_some() || self.is_optional_type(&param.ty),
                    })
                    .collect();
                let arg_specs: Vec<CallArgSpec<'_>> = args
                    .keys()
                    .map(|name| CallArgSpec {
                        name: Some(name.as_str()),
                    })
                    .collect();
                let (plan, bind_errors) = bind_call_args(&param_specs, &arg_specs);
                if let Some(err) = bind_errors.first() {
                    return Err(self.format_named_call_bind_error(err));
                }

                for (idx, param) in decl.params.iter().enumerate() {
                    let value = match plan.param_bindings.get(idx) {
                        Some(ParamBinding::Arg(_)) => args
                            .get(&param.name.name)
                            .cloned()
                            .expect("bound named argument should exist"),
                        Some(ParamBinding::Default) => {
                            if let Some(default) = &param.default {
                                self.eval_expr(default)
                                    .map_err(|err| self.render_exec_error(err))?
                            } else if self.is_optional_type(&param.ty) {
                                Value::Null
                            } else {
                                return Err(format!("missing argument {}", param.name.name));
                            }
                        }
                        Some(ParamBinding::MissingRequired) | None => {
                            return Err(format!("missing argument {}", param.name.name));
                        }
                    };
                    self.validate_value(&value, &param.ty, &param.name.name)
                        .map_err(|err| self.render_exec_error(err))?;
                    self.env.insert(&param.name.name, value);
                }

                let result = self.eval_block(&decl.body);
                match result {
                    Ok(value) => Ok(value),
                    Err(ExecError::Return(value)) => Ok(value),
                    Err(err) => Err(self.render_exec_error(err)),
                }
            })();
            self.env.pop();
            result
        })();
        self.current_module = prev_module;
        out
    }

    fn format_named_call_bind_error(&self, err: &CallBindError) -> String {
        match err {
            CallBindError::UnknownArgument(name) => format!("unknown argument {}", name),
            CallBindError::DuplicateArgument(name) => format!("duplicate argument {}", name),
            CallBindError::TooManyArguments => "too many arguments".to_string(),
        }
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
        let decls: Vec<ConfigDecl> = self.config_decls.values().cloned().collect();
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
            StmtKind::Transaction { block } => {
                {
                    let db = self.db_mut()?;
                    db.begin_transaction().map_err(ExecError::Runtime)?;
                }
                match self.eval_block(block) {
                    Ok(_) => {
                        let db = self.db_mut()?;
                        if let Err(err) = db.commit_transaction() {
                            let _ = db.rollback_transaction();
                            return Err(ExecError::Runtime(err));
                        }
                        Ok(Value::Unit)
                    }
                    Err(err) => {
                        if let Ok(db) = self.db_mut() {
                            let _ = db.rollback_transaction();
                        }
                        Err(err)
                    }
                }
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
                if let ExprKind::Ident(ident) = &callee.kind {
                    if self.should_use_html_tag_builtin(&ident.name)
                        || force_html_input_tag_call(&ident.name, args)
                    {
                        if let Some(message) = self.validate_html_tag_named_args(args) {
                            return Err(ExecError::Runtime(message.to_string()));
                        }
                        let mut arg_vals = Vec::with_capacity(args.len());
                        for arg in args {
                            arg_vals.push(self.eval_expr(&arg.value)?);
                        }
                        return self.eval_builtin(&ident.name, arg_vals);
                    }
                }
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
                let mut worker = self.spawn_worker();
                let captured_env = self.env.clone();
                let current_module = self.current_module;
                let block = block.clone();
                let task = Task::spawn_async(move || {
                    worker.env = captured_env;
                    worker.current_module = current_module;
                    Task::from_exec_result(worker.eval_block(&block))
                });
                Ok(Value::Task(task))
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
                    other => Ok(Value::Boxed(Arc::new(Mutex::new(other)))),
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

    fn db_pool_size(&self) -> ExecResult<usize> {
        if let Ok(raw) = std::env::var("FUSE_DB_POOL_SIZE") {
            return parse_db_pool_size(&raw, "FUSE_DB_POOL_SIZE").map_err(ExecError::Runtime);
        }
        if let Some(value) = self
            .configs
            .get("App")
            .and_then(|config| config.get("dbPoolSize"))
        {
            return parse_db_pool_size_value(value, "App.dbPoolSize").map_err(ExecError::Runtime);
        }
        Ok(DEFAULT_DB_POOL_SIZE)
    }

    fn db_mut(&mut self) -> ExecResult<&mut Db> {
        if self.db.is_none() {
            let pool_size = self.db_pool_size()?;
            let url = self.db_url()?;
            let db = Db::open_with_pool(&url, pool_size).map_err(ExecError::Runtime)?;
            self.db = Some(db);
        }
        Ok(self.db.as_mut().expect("db initialized"))
    }

    fn resolve_ident(&self, name: &str) -> ExecResult<Value> {
        if let Some(val) = self.env.get(name) {
            return Ok(val);
        }
        if let Some(func) = self.resolve_function_ref(self.current_module, name) {
            return Ok(Value::Function(func));
        }
        if self.config_decls.contains_key(name) {
            return Ok(Value::Config(name.to_string()));
        }
        match name {
            "print" | "input" | "env" | "serve" | "log" | "db" | "assert" | "asset" | "json"
            | "html" | "svg" | "request" | "response" => Ok(Value::Builtin(name.to_string())),
            _ if html_tags::is_html_tag(name) => Ok(Value::Builtin(name.to_string())),
            _ => Err(ExecError::Runtime(format!("unknown identifier {name}"))),
        }
    }

    fn should_use_html_tag_builtin(&self, name: &str) -> bool {
        should_use_html_tag_builtin(
            name,
            self.env.get(name).is_some(),
            self.resolve_function_ref(self.current_module, name)
                .is_some(),
            self.config_decls.contains_key(name),
            name == "input",
        )
    }

    fn validate_html_tag_named_args(&self, args: &[crate::ast::CallArg]) -> Option<&'static str> {
        validate_named_args_for_phase(args, CanonicalizationPhase::Execution)
    }

    fn eval_call(&mut self, callee: Value, args: Vec<Value>) -> ExecResult<Value> {
        match callee.unboxed() {
            Value::Builtin(name) => self.eval_builtin(&name, args),
            Value::Function(func) => self.eval_function(&func, args),
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
        if html_tags::is_html_tag(name) && name != "input" {
            return self.eval_html_tag_builtin(name, &args);
        }
        match name {
            "print" => {
                let text = args.get(0).map(|v| v.to_string_value()).unwrap_or_default();
                println!("{text}");
                Ok(Value::Unit)
            }
            "input" => {
                if args.len() > 1 {
                    return Err(ExecError::Runtime(
                        "input expects 0 or 1 arguments".to_string(),
                    ));
                }
                let prompt = match args.first() {
                    Some(Value::String(text)) => text.as_str(),
                    Some(_) => {
                        return Err(ExecError::Runtime(
                            "input expects a string prompt".to_string(),
                        ));
                    }
                    None => "",
                };
                let text =
                    crate::runtime_io::read_input_line(prompt).map_err(ExecError::Runtime)?;
                Ok(Value::String(text))
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
                        eprintln!(
                            "{}",
                            crate::runtime_io::format_log_text_line(level.label(), &message)
                        );
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
            "asset" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime("asset expects 1 argument".to_string()));
                }
                let path = match args.get(0) {
                    Some(Value::String(path)) => path,
                    _ => {
                        return Err(ExecError::Runtime(
                            "asset expects a string path".to_string(),
                        ));
                    }
                };
                Ok(Value::String(crate::runtime_assets::resolve_asset_href(
                    path,
                )))
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
            "svg.inline" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "svg.inline expects 1 argument".to_string(),
                    ));
                }
                let name = match args.get(0) {
                    Some(Value::String(name)) => name,
                    _ => {
                        return Err(ExecError::Runtime(
                            "svg.inline expects a String path".to_string(),
                        ));
                    }
                };
                let svg = crate::runtime_svg::load_svg_inline(name).map_err(ExecError::Runtime)?;
                Ok(Value::Html(HtmlNode::Raw(svg)))
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
            "db.tx_begin" => {
                if !args.is_empty() {
                    return Err(ExecError::Runtime(
                        "db.tx_begin expects no arguments".to_string(),
                    ));
                }
                let db = self.db_mut()?;
                db.begin_transaction().map_err(ExecError::Runtime)?;
                Ok(Value::Unit)
            }
            "db.tx_commit" => {
                if !args.is_empty() {
                    return Err(ExecError::Runtime(
                        "db.tx_commit expects no arguments".to_string(),
                    ));
                }
                let db = self.db_mut()?;
                if let Err(err) = db.commit_transaction() {
                    let _ = db.rollback_transaction();
                    return Err(ExecError::Runtime(err));
                }
                Ok(Value::Unit)
            }
            "db.tx_rollback" => {
                if !args.is_empty() {
                    return Err(ExecError::Runtime(
                        "db.tx_rollback expects no arguments".to_string(),
                    ));
                }
                let db = self.db_mut()?;
                db.rollback_transaction().map_err(ExecError::Runtime)?;
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
            "request.header" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "request.header expects 1 argument".to_string(),
                    ));
                }
                let name = match args.first() {
                    Some(Value::String(name)) => name,
                    _ => {
                        return Err(ExecError::Runtime(
                            "request.header expects a string name".to_string(),
                        ));
                    }
                };
                let value = self.request_header(name)?;
                match value {
                    Some(value) => Ok(Value::String(value)),
                    None => Ok(Value::Null),
                }
            }
            "request.cookie" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "request.cookie expects 1 argument".to_string(),
                    ));
                }
                let name = match args.first() {
                    Some(Value::String(name)) => name,
                    _ => {
                        return Err(ExecError::Runtime(
                            "request.cookie expects a string name".to_string(),
                        ));
                    }
                };
                let value = self.request_cookie(name)?;
                match value {
                    Some(value) => Ok(Value::String(value)),
                    None => Ok(Value::Null),
                }
            }
            "response.header" => {
                if args.len() != 2 {
                    return Err(ExecError::Runtime(
                        "response.header expects 2 arguments".to_string(),
                    ));
                }
                let (name, value) = match (args.first(), args.get(1)) {
                    (Some(Value::String(name)), Some(Value::String(value))) => {
                        (name.as_str(), value.as_str())
                    }
                    _ => {
                        return Err(ExecError::Runtime(
                            "response.header expects string name and value".to_string(),
                        ));
                    }
                };
                self.response_add_header(name, value)?;
                Ok(Value::Unit)
            }
            "response.cookie" => {
                if args.len() != 2 {
                    return Err(ExecError::Runtime(
                        "response.cookie expects 2 arguments".to_string(),
                    ));
                }
                let (name, value) = match (args.first(), args.get(1)) {
                    (Some(Value::String(name)), Some(Value::String(value))) => {
                        (name.as_str(), value.as_str())
                    }
                    _ => {
                        return Err(ExecError::Runtime(
                            "response.cookie expects string name and value".to_string(),
                        ));
                    }
                };
                self.response_set_cookie(name, value)?;
                Ok(Value::Unit)
            }
            "response.delete_cookie" => {
                if args.len() != 1 {
                    return Err(ExecError::Runtime(
                        "response.delete_cookie expects 1 argument".to_string(),
                    ));
                }
                let name = match args.first() {
                    Some(Value::String(name)) => name,
                    _ => {
                        return Err(ExecError::Runtime(
                            "response.delete_cookie expects a string name".to_string(),
                        ));
                    }
                };
                self.response_delete_cookie(name)?;
                Ok(Value::Unit)
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

    fn eval_html_tag_builtin(&self, name: &str, args: &[Value]) -> ExecResult<Value> {
        let Some(kind) = html_tags::tag_kind(name) else {
            return Err(ExecError::Runtime(format!("unknown builtin {name}")));
        };
        let max = match kind {
            HtmlTagKind::Normal => 2usize,
            HtmlTagKind::Void => 1usize,
        };
        if args.len() > max {
            return Err(ExecError::Runtime(format!(
                "{} expects at most {} arguments",
                name, max
            )));
        }
        let attrs = match args.get(0) {
            Some(Value::Map(map)) => {
                let mut attrs = HashMap::with_capacity(map.len());
                for (key, value) in map {
                    let Value::String(text) = value else {
                        return Err(ExecError::Runtime(format!(
                            "{} attrs must be Map<String, String>",
                            name
                        )));
                    };
                    attrs.insert(key.clone(), text.clone());
                }
                attrs
            }
            Some(_) => {
                return Err(ExecError::Runtime(format!(
                    "{} expects attrs as Map<String, String>",
                    name
                )));
            }
            None => HashMap::new(),
        };
        let children = match kind {
            HtmlTagKind::Void => Vec::new(),
            HtmlTagKind::Normal => match args.get(1) {
                Some(Value::List(items)) => {
                    let mut children = Vec::with_capacity(items.len());
                    for item in items {
                        let Value::Html(node) = item else {
                            return Err(ExecError::Runtime(format!(
                                "{} children must be List<Html>",
                                name
                            )));
                        };
                        children.push(node.clone());
                    }
                    children
                }
                Some(_) => {
                    return Err(ExecError::Runtime(format!(
                        "{} expects children as List<Html>",
                        name
                    )));
                }
                None => Vec::new(),
            },
        };
        Ok(Value::Html(HtmlNode::Element {
            tag: name.to_string(),
            attrs,
            children,
        }))
    }

    fn eval_function(&mut self, func: &FunctionRef, args: Vec<Value>) -> ExecResult<Value> {
        let decl = match self.function_decl(func) {
            Some(decl) => decl.clone(),
            None => {
                return Err(ExecError::Runtime(format!(
                    "unknown function {}::{}",
                    func.module_id, func.name
                )));
            }
        };
        let param_specs: Vec<ParamSpec<'_>> = decl
            .params
            .iter()
            .map(|param| ParamSpec {
                name: &param.name.name,
                has_default: param.default.is_some(),
            })
            .collect();
        let (plan, bind_errors) = bind_positional_args(&param_specs, args.len());
        if !bind_errors.is_empty() {
            return Err(ExecError::Runtime(format!(
                "invalid call to {}: expected at most {} args, got {}",
                func.name,
                decl.params.len(),
                args.len()
            )));
        }

        let prev_module = self.current_module;
        self.current_module = func.module_id;
        let out = (|| -> ExecResult<Value> {
            self.env.push();
            let result = (|| -> ExecResult<Value> {
                for (idx, param) in decl.params.iter().enumerate() {
                    let value = match plan.param_bindings.get(idx) {
                        Some(ParamBinding::Arg(arg_idx)) => args[*arg_idx].clone(),
                        Some(ParamBinding::Default) => {
                            if let Some(default) = &param.default {
                                self.eval_expr(default)?
                            } else {
                                return Err(ExecError::Runtime(format!(
                                    "missing argument {}",
                                    param.name.name
                                )));
                            }
                        }
                        Some(ParamBinding::MissingRequired) | None => {
                            return Err(ExecError::Runtime(format!(
                                "missing argument {}",
                                param.name.name
                            )));
                        }
                    };
                    self.env.insert(&param.name.name, value);
                }

                let result = self.eval_block(&decl.body);
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
                    Err(ExecError::Break) => {
                        Err(ExecError::Runtime("break outside of loop".to_string()))
                    }
                    Err(ExecError::Continue) => {
                        Err(ExecError::Runtime("continue outside of loop".to_string()))
                    }
                }
            })();
            self.env.pop();
            result
        })();
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
            let request = match self.read_http_request(&mut stream) {
                Ok(request) => request,
                Err(err) => {
                    let response = self.http_error_response(err);
                    let _ = stream.write_all(response.as_bytes());
                    handled += 1;
                    if max_requests > 0 && handled >= max_requests {
                        break;
                    }
                    continue;
                }
            };
            let started = Instant::now();
            let response = match self.handle_http_request(&service, &request) {
                Ok(resp) => resp,
                Err(err) => self.http_error_response_for_request(&request, err),
            };
            let (status, response_bytes) =
                observability::parse_http_response_status_and_body_len(&response);
            observability::emit_http_observability(
                "ast",
                &request.request_id,
                &request.method,
                &request.path,
                status,
                started.elapsed(),
                response_bytes,
            );
            let _ = stream.write_all(response.as_bytes());
            handled += 1;
            if max_requests > 0 && handled >= max_requests {
                break;
            }
        }
        self.current_module = prev_module;
        Ok(Value::Unit)
    }

    fn select_service(&self) -> ExecResult<&ServiceDecl> {
        if self.services.is_empty() {
            return Err(ExecError::Runtime("no service declared".to_string()));
        }
        if let Ok(name) = std::env::var("FUSE_SERVICE") {
            return self
                .services
                .get(&name)
                .ok_or_else(|| ExecError::Runtime(format!("service not found: {name}")));
        }
        if self.services.len() == 1 {
            return Ok(self.services.values().next().unwrap());
        }
        Err(ExecError::Runtime(
            "multiple services declared; set FUSE_SERVICE".to_string(),
        ))
    }

    fn handle_http_request(
        &mut self,
        service: &ServiceDecl,
        request: &HttpRequest,
    ) -> ExecResult<String> {
        let verb = match request.method.as_str() {
            "GET" => HttpVerb::Get,
            "POST" => HttpVerb::Post,
            "PUT" => HttpVerb::Put,
            "PATCH" => HttpVerb::Patch,
            "DELETE" => HttpVerb::Delete,
            _ => {
                return Ok(self.http_response_for_request(
                    request,
                    405,
                    self.internal_error_json("method not allowed"),
                ));
            }
        };
        let path = request
            .path
            .split('?')
            .next()
            .unwrap_or(&request.path)
            .to_string();
        if let Some(response) = self.try_openapi_ui_response(request.method.as_str(), &path) {
            return Ok(observability::inject_request_id_header(
                response,
                &request.request_id,
            ));
        }
        if let Some(response) = self.try_static_response(request.method.as_str(), &path) {
            return Ok(observability::inject_request_id_header(
                response,
                &request.request_id,
            ));
        }
        let (route, params) = match self.match_route(service, &verb, &path)? {
            Some(result) => result,
            None => {
                if let Some(response) = self.try_vite_proxy_response(request) {
                    return Ok(observability::inject_request_id_header(
                        response,
                        &request.request_id,
                    ));
                }
                let body = self.error_json_from_code("not_found", "not found");
                return Ok(self.http_response_for_request(request, 404, body));
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
        self.begin_http_route_context(&request);
        let value = self.eval_route(route, params, body_value);
        let response_meta = self.end_http_route_context();
        let value = match value {
            Ok(value) => value,
            Err(err) => return Err(err),
        };
        let html_response = is_html_response_type(&route.ret_type);
        match value {
            Value::ResultErr(err) => {
                let status = self.http_status_for_error_value(&err);
                let json = self.error_json_from_value(&err);
                Ok(self.http_response_with_meta(
                    status,
                    json,
                    "application/json",
                    Some(&response_meta),
                ))
            }
            Value::ResultOk(ok) => {
                if html_response {
                    let body =
                        self.maybe_inject_live_reload_html(self.render_html_value(ok.as_ref())?);
                    Ok(self.http_response_with_meta(
                        200,
                        body,
                        "text/html; charset=utf-8",
                        Some(&response_meta),
                    ))
                } else {
                    let json = self.value_to_json(&ok);
                    Ok(self.http_response_with_meta(
                        200,
                        rt_json::encode(&json),
                        "application/json",
                        Some(&response_meta),
                    ))
                }
            }
            other => {
                if html_response {
                    let body = self.maybe_inject_live_reload_html(self.render_html_value(&other)?);
                    Ok(self.http_response_with_meta(
                        200,
                        body,
                        "text/html; charset=utf-8",
                        Some(&response_meta),
                    ))
                } else {
                    let json = self.value_to_json(&other);
                    Ok(self.http_response_with_meta(
                        200,
                        rt_json::encode(&json),
                        "application/json",
                        Some(&response_meta),
                    ))
                }
            }
        }
    }

    fn begin_http_route_context(&mut self, request: &HttpRequest) {
        self.current_http_request = Some(HttpRequestContext {
            headers: request.headers.clone(),
            cookies: parse_cookie_map(request.headers.get("cookie").map(String::as_str)),
        });
        self.current_http_response = Some(HttpResponseMeta {
            request_id: Some(request.request_id.clone()),
            ..HttpResponseMeta::default()
        });
    }

    fn end_http_route_context(&mut self) -> HttpResponseMeta {
        self.current_http_request = None;
        self.current_http_response.take().unwrap_or_default()
    }

    fn request_header(&self, name: &str) -> ExecResult<Option<String>> {
        let request = self.current_http_request.as_ref().ok_or_else(|| {
            ExecError::Runtime(
                "request.header is only available while handling an HTTP route".to_string(),
            )
        })?;
        Ok(request.headers.get(&name.to_ascii_lowercase()).cloned())
    }

    fn request_cookie(&self, name: &str) -> ExecResult<Option<String>> {
        let request = self.current_http_request.as_ref().ok_or_else(|| {
            ExecError::Runtime(
                "request.cookie is only available while handling an HTTP route".to_string(),
            )
        })?;
        Ok(request.cookies.get(name).cloned())
    }

    fn response_add_header(&mut self, name: &str, value: &str) -> ExecResult<()> {
        validate_http_header(name, value)?;
        let response = self.current_http_response.as_mut().ok_or_else(|| {
            ExecError::Runtime(
                "response.header is only available while handling an HTTP route".to_string(),
            )
        })?;
        response.headers.push((name.to_string(), value.to_string()));
        Ok(())
    }

    fn response_set_cookie(&mut self, name: &str, value: &str) -> ExecResult<()> {
        validate_cookie_name(name)?;
        validate_cookie_value(value)?;
        let response = self.current_http_response.as_mut().ok_or_else(|| {
            ExecError::Runtime(
                "response.cookie is only available while handling an HTTP route".to_string(),
            )
        })?;
        response
            .cookies
            .push(format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax"));
        Ok(())
    }

    fn response_delete_cookie(&mut self, name: &str) -> ExecResult<()> {
        validate_cookie_name(name)?;
        let response = self.current_http_response.as_mut().ok_or_else(|| {
            ExecError::Runtime(
                "response.delete_cookie is only available while handling an HTTP route".to_string(),
            )
        })?;
        response.cookies.push(format!(
            "{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
        ));
        Ok(())
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
        let mut body = fs::read_to_string(&full).ok()?;
        let content_type = match full.extension().and_then(|ext| ext.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("js") => "application/javascript; charset=utf-8",
            Some("json") => "application/json; charset=utf-8",
            _ => "text/plain; charset=utf-8",
        };
        if content_type.starts_with("text/html") {
            body = self.maybe_inject_live_reload_html(body);
        }
        Some(self.http_response_with_type(200, body, content_type))
    }

    fn try_vite_proxy_response(&self, request: &HttpRequest) -> Option<String> {
        let base_url = std::env::var("FUSE_VITE_PROXY_URL").ok()?;
        proxy_http_request(request, &base_url)
    }

    fn try_openapi_ui_response(&self, method: &str, path: &str) -> Option<String> {
        if method != "GET" {
            return None;
        }
        let spec_path = std::env::var("FUSE_OPENAPI_JSON_PATH").ok()?;
        let ui_path = normalize_openapi_ui_path(
            std::env::var("FUSE_OPENAPI_UI_PATH")
                .ok()
                .as_deref()
                .unwrap_or("/docs"),
        );
        let path_no_slash = path.strip_suffix('/').unwrap_or(path);
        let docs_path_no_slash = ui_path.strip_suffix('/').unwrap_or(&ui_path);
        if path_no_slash == docs_path_no_slash {
            let spec_url = format!("{docs_path_no_slash}/openapi.json");
            let body = self.maybe_inject_live_reload_html(openapi_ui_html(&spec_url));
            return Some(self.http_response_with_type(200, body, "text/html; charset=utf-8"));
        }
        let spec_route = format!("{docs_path_no_slash}/openapi.json");
        if path == spec_route {
            let body = match fs::read_to_string(&spec_path) {
                Ok(body) => body,
                Err(err) => {
                    return Some(self.http_response(
                        500,
                        self.internal_error_json(&format!("failed to read openapi spec: {err}")),
                    ));
                }
            };
            return Some(self.http_response_with_type(
                200,
                body,
                "application/json; charset=utf-8",
            ));
        }
        None
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
        let request_id = observability::resolve_request_id(&headers);
        headers.insert(
            observability::REQUEST_ID_HEADER.to_string(),
            request_id.clone(),
        );
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
        Ok(HttpRequest {
            method,
            path,
            request_id,
            headers,
            body,
        })
    }

    fn http_response(&self, status: u16, body: String) -> String {
        self.http_response_with_meta(status, body, "application/json", None)
    }

    fn http_response_for_request(
        &self,
        request: &HttpRequest,
        status: u16,
        body: String,
    ) -> String {
        self.http_response_with_type_for_request(request, status, body, "application/json")
    }

    fn http_response_with_type(&self, status: u16, body: String, content_type: &str) -> String {
        self.http_response_with_meta(status, body, content_type, None)
    }

    fn http_response_with_type_for_request(
        &self,
        request: &HttpRequest,
        status: u16,
        body: String,
        content_type: &str,
    ) -> String {
        let mut meta = HttpResponseMeta::default();
        meta.request_id = Some(request.request_id.clone());
        self.http_response_with_meta(status, body, content_type, Some(&meta))
    }

    fn http_response_with_meta(
        &self,
        status: u16,
        body: String,
        content_type: &str,
        meta: Option<&HttpResponseMeta>,
    ) -> String {
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
        let mut response =
            format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n");
        if let Some(meta) = meta {
            if let Some(request_id) = meta.request_id.as_deref() {
                response.push_str(&format!(
                    "{}: {request_id}\r\n",
                    observability::RESPONSE_REQUEST_ID_HEADER
                ));
            }
            for (name, value) in &meta.headers {
                if name.eq_ignore_ascii_case(observability::REQUEST_ID_HEADER) {
                    continue;
                }
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            for cookie in &meta.cookies {
                response.push_str(&format!("Set-Cookie: {cookie}\r\n"));
            }
        }
        response.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        response
    }

    fn maybe_inject_live_reload_html(&self, mut body: String) -> String {
        let ws_url = match std::env::var("FUSE_DEV_RELOAD_WS_URL") {
            Ok(url) if !url.trim().is_empty() => url,
            _ => return body,
        };
        if body.contains("data-fuse-live-reload") {
            return body;
        }
        let ws_url = escape_js_single_quoted(&ws_url);
        let script = format!(
            "<script data-fuse-live-reload>(function(){{var url='{ws_url}';var retry=500;function connect(){{var ws=new WebSocket(url);ws.onopen=function(){{retry=500;}};ws.onmessage=function(){{window.location.reload();}};ws.onclose=function(){{setTimeout(connect,retry);retry=Math.min(retry*2,3000);}};ws.onerror=function(){{ws.close();}};}}connect();}})();</script>"
        );
        if let Some(index) = body.rfind("</body>") {
            body.insert_str(index, &script);
        } else {
            body.push_str(&script);
        }
        body
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

    fn http_error_response_for_request(&self, request: &HttpRequest, err: ExecError) -> String {
        match err {
            ExecError::Error(value) => {
                let status = self.http_status_for_error_value(&value);
                let body = self.error_json_from_value(&value);
                self.http_response_for_request(request, status, body)
            }
            ExecError::Runtime(message) => {
                let body = self.internal_error_json(&message);
                self.http_response_for_request(request, 500, body)
            }
            ExecError::Return(_) => self.http_response_for_request(
                request,
                500,
                self.internal_error_json("unexpected return"),
            ),
            ExecError::Break => self.http_response_for_request(
                request,
                500,
                self.internal_error_json("break outside loop"),
            ),
            ExecError::Continue => self.http_response_for_request(
                request,
                500,
                self.internal_error_json("continue outside loop"),
            ),
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
            Value::Builtin(name) if name == "html" => match field {
                "text" | "raw" | "node" | "render" => Ok(Value::Builtin(format!("html.{field}"))),
                _ => Err(ExecError::Runtime(format!("unknown html method {field}"))),
            },
            Value::Builtin(name) if name == "svg" => match field {
                "inline" => Ok(Value::Builtin(format!("svg.{field}"))),
                _ => Err(ExecError::Runtime(format!("unknown svg method {field}"))),
            },
            Value::Builtin(name) if name == "request" => match field {
                "header" | "cookie" => Ok(Value::Builtin(format!("request.{field}"))),
                _ => Err(ExecError::Runtime(format!(
                    "unknown request method {field}"
                ))),
            },
            Value::Builtin(name) if name == "response" => match field {
                "header" | "cookie" | "delete_cookie" => {
                    Ok(Value::Builtin(format!("response.{field}")))
                }
                _ => Err(ExecError::Runtime(format!(
                    "unknown response method {field}"
                ))),
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
                    return Ok(Some(Value::Function(FunctionRef {
                        module_id: module.id,
                        name: field.to_string(),
                    })));
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
            Some(decl) => decl.clone(),
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
                    *cell.lock().expect("box lock") = value.unboxed();
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
                let mut inner = cell.lock().expect("box lock");
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
            (Value::Bool(a), Value::Bool(b)) => match op {
                BinaryOp::Eq => a == b,
                BinaryOp::NotEq => a != b,
                _ => {
                    return Err(ExecError::Runtime(
                        "unsupported bool comparison".to_string(),
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
        crate::runtime_types::parse_env_value(self, ty, raw)
    }

    fn is_optional_type(&self, ty: &TypeRef) -> bool {
        crate::runtime_types::is_optional_type(ty)
    }

    fn validate_value(&mut self, value: &Value, ty: &TypeRef, path: &str) -> ExecResult<()> {
        crate::runtime_types::validate_value(self, value, ty, path)
    }

    fn value_type_name(&self, value: &Value) -> String {
        crate::runtime_types::value_type_name(value)
    }

    fn value_to_json(&self, value: &Value) -> rt_json::JsonValue {
        crate::runtime_types::value_to_json(value)
    }

    fn json_to_value(&self, json: &rt_json::JsonValue) -> Value {
        crate::runtime_types::json_to_value(json)
    }

    fn decode_json_value(
        &mut self,
        json: &rt_json::JsonValue,
        ty: &TypeRef,
        path: &str,
    ) -> ExecResult<Value> {
        crate::runtime_types::decode_json_value(self, json, ty, path)
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
        &mut self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> ExecResult<()> {
        let constraints = parse_constraints(args).map_err(|err| {
            ExecError::Runtime(format!("invalid refined constraint: {}", err.message))
        })?;
        for constraint in constraints {
            match constraint {
                RefinementConstraint::Range { min, max, .. } => {
                    self.check_refined_range(value, base, min, max, path)?;
                }
                RefinementConstraint::Regex { pattern, .. } => {
                    if !base_is_string_like(base) {
                        return Err(ExecError::Runtime(format!(
                            "regex() constraint is not supported for refined {base}"
                        )));
                    }
                    let Value::String(text) = value.unboxed() else {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected String"
                        )));
                    };
                    if !self.regex_matches(&pattern, &text)? {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            format!("value does not match regex {pattern}"),
                        )));
                    }
                }
                RefinementConstraint::Predicate { name, .. } => {
                    if !self.eval_refinement_predicate(&name, value)? {
                        return Err(ExecError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            format!("predicate {name} rejected value"),
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn check_refined_range(
        &self,
        value: &Value,
        base: &str,
        min: NumberLiteral,
        max: NumberLiteral,
        path: &str,
    ) -> ExecResult<()> {
        match base {
            "String" | "Id" | "Email" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
                let len = match value.unboxed() {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected String"
                        )));
                    }
                };
                if rt_validate::check_len(len, min, max) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("length {len} out of range {min}..{max}"),
                    )))
                }
            }
            "Bytes" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
                let len = match value.unboxed() {
                    Value::Bytes(bytes) => bytes.len() as i64,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Bytes"
                        )));
                    }
                };
                if rt_validate::check_len(len, min, max) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("length {len} out of range {min}..{max}"),
                    )))
                }
            }
            "Int" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| ExecError::Runtime("invalid refined range".to_string()))?;
                let val = match value.unboxed() {
                    Value::Int(v) => v,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Int"
                        )));
                    }
                };
                if rt_validate::check_int_range(val, min, max) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("value {val} out of range {min}..{max}"),
                    )))
                }
            }
            "Float" => {
                let min = min.as_f64();
                let max = max.as_f64();
                let val = match value.unboxed() {
                    Value::Float(v) => v,
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "type mismatch at {path}: expected Float"
                        )));
                    }
                };
                if rt_validate::check_float_range(val, min, max) {
                    Ok(())
                } else {
                    Err(ExecError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("value {val} out of range {min}..{max}"),
                    )))
                }
            }
            _ => Err(ExecError::Runtime(format!(
                "range constraint is not supported for refined {base}"
            ))),
        }
    }

    fn regex_matches(&mut self, pattern: &str, text: &str) -> ExecResult<bool> {
        if !self.regex_cache.contains_key(pattern) {
            let compiled = regex::Regex::new(pattern).map_err(|err| {
                ExecError::Runtime(format!("invalid regex pattern {pattern}: {err}"))
            })?;
            self.regex_cache.insert(pattern.to_string(), compiled);
        }
        let regex = self
            .regex_cache
            .get(pattern)
            .ok_or_else(|| ExecError::Runtime("regex cache error".to_string()))?;
        Ok(regex.is_match(text))
    }

    fn eval_refinement_predicate(&mut self, fn_name: &str, value: &Value) -> ExecResult<bool> {
        let func_ref = self
            .resolve_function_ref(self.current_module, fn_name)
            .ok_or_else(|| {
                ExecError::Runtime(format!(
                    "unknown predicate function {fn_name} in current module/import scope"
                ))
            })?;
        let decl = self
            .function_decl(&func_ref)
            .ok_or_else(|| {
                ExecError::Runtime(format!(
                    "unknown function {}::{}",
                    func_ref.module_id, func_ref.name
                ))
            })?
            .clone();
        if decl.params.len() != 1 {
            return Err(ExecError::Runtime(format!(
                "predicate {fn_name} must accept exactly one argument"
            )));
        }
        let prev_module = self.current_module;
        self.current_module = func_ref.module_id;
        self.env.push();
        let param = &decl.params[0];
        if let Err(err) = self.validate_value(value, &param.ty, &param.name.name) {
            self.env.pop();
            self.current_module = prev_module;
            return Err(err);
        }
        self.env.insert(&param.name.name, value.clone());
        let result = self.eval_block(&decl.body);
        self.env.pop();
        self.current_module = prev_module;
        let out = match result {
            Ok(value) | Err(ExecError::Return(value)) => value,
            Err(err) => return Err(err),
        };
        match out.unboxed() {
            Value::Bool(ok) => Ok(ok),
            _ => Err(ExecError::Runtime(format!(
                "predicate {fn_name} must return Bool"
            ))),
        }
    }

    fn validation_error_value(&self, path: &str, code: &str, message: impl Into<String>) -> Value {
        crate::runtime_types::validation_error_value(path, code, message)
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

#[derive(Clone, Default)]
struct HttpRequestContext {
    headers: HashMap<String, String>,
    cookies: HashMap<String, String>,
}

#[derive(Clone, Default)]
struct HttpResponseMeta {
    request_id: Option<String>,
    headers: Vec<(String, String)>,
    cookies: Vec<String>,
}

struct HttpRequest {
    method: String,
    path: String,
    request_id: String,
    headers: HashMap<String, String>,
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

fn parse_cookie_map(raw: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(raw) = raw else {
        return out;
    };
    for part in raw.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        out.insert(name.to_string(), value.trim().to_string());
    }
    out
}

fn validate_http_header(name: &str, value: &str) -> ExecResult<()> {
    if name.trim().is_empty() {
        return Err(ExecError::Runtime(
            "response.header requires a non-empty name".to_string(),
        ));
    }
    if name.contains('\r') || name.contains('\n') || value.contains('\r') || value.contains('\n') {
        return Err(ExecError::Runtime(
            "response.header rejects CR/LF characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_cookie_name(name: &str) -> ExecResult<()> {
    if name.is_empty() {
        return Err(ExecError::Runtime(
            "cookie name must not be empty".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(ExecError::Runtime(
            "cookie name contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_cookie_value(value: &str) -> ExecResult<()> {
    if value.contains(';') || value.contains('\r') || value.contains('\n') {
        return Err(ExecError::Runtime(
            "cookie value contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn proxy_http_request(request: &HttpRequest, base_url: &str) -> Option<String> {
    let (host, port, base_path) = parse_proxy_base_url(base_url)?;
    let request_path = if request.path.starts_with('/') {
        request.path.clone()
    } else {
        format!("/{}", request.path)
    };
    let target_path = join_proxy_paths(&base_path, &request_path);
    let mut upstream = TcpStream::connect((host.as_str(), port)).ok()?;
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
        request.method,
        target_path,
        request.body.len()
    );
    if !request.body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if let Some(request_id) = request.headers.get(observability::REQUEST_ID_HEADER) {
        head.push_str(&format!(
            "{}: {request_id}\r\n",
            observability::RESPONSE_REQUEST_ID_HEADER
        ));
    }
    head.push_str("\r\n");
    upstream.write_all(head.as_bytes()).ok()?;
    if !request.body.is_empty() {
        upstream.write_all(&request.body).ok()?;
    }
    let mut response = Vec::new();
    upstream.read_to_end(&mut response).ok()?;
    Some(String::from_utf8_lossy(&response).into_owned())
}

fn parse_proxy_base_url(raw: &str) -> Option<(String, u16, String)> {
    let trimmed = raw.trim();
    let rest = trimmed.strip_prefix("http://")?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, tail)) => (authority, format!("/{}", tail.trim_start_matches('/'))),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().ok()?;
            (host.to_string(), port)
        }
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return None;
    }
    Some((host, port, path))
}

fn join_proxy_paths(base_path: &str, request_path: &str) -> String {
    if base_path == "/" {
        return request_path.to_string();
    }
    if request_path == "/" {
        return base_path.to_string();
    }
    format!(
        "{}/{}",
        base_path.trim_end_matches('/'),
        request_path.trim_start_matches('/')
    )
}

fn escape_js_single_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

fn normalize_openapi_ui_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "/docs".to_string();
    }
    let mut path = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    path
}

fn openapi_ui_html(spec_url: &str) -> String {
    let spec_url = escape_js_single_quoted(spec_url);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>OpenAPI</title><style>:root{{color-scheme:light dark;font-family:ui-sans-serif,system-ui,sans-serif}}body{{margin:0;padding:24px;background:#0b1020;color:#e6e8ee}}main{{max-width:980px;margin:0 auto}}h1{{margin:0 0 12px;font-size:1.6rem}}.muted{{color:#9aa3b2;font-size:.92rem}}.card{{margin-top:16px;padding:16px;border:1px solid #27314a;border-radius:12px;background:#121a2c}}.route{{padding:8px 0;border-bottom:1px solid #222b43}}.route:last-child{{border-bottom:0}}.method{{display:inline-block;min-width:56px;font-weight:700}}code{{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}}</style></head><body><main><h1>FUSE OpenAPI</h1><div class=\"muted\">spec: <code id=\"spec-url\"></code></div><section id=\"status\" class=\"card\">Loading…</section><section id=\"routes\" class=\"card\" hidden><h2>Routes</h2><div id=\"route-list\"></div></section></main><script>(function(){{var specUrl='{spec_url}';document.getElementById('spec-url').textContent=specUrl;var status=document.getElementById('status');var routes=document.getElementById('routes');var list=document.getElementById('route-list');fetch(specUrl).then(function(res){{if(!res.ok){{throw new Error('HTTP '+res.status);}}return res.json();}}).then(function(doc){{status.textContent='Loaded '+(doc.info&&doc.info.title?doc.info.title:'OpenAPI')+' '+((doc.info&&doc.info.version)||'');var paths=doc.paths||{{}};var entries=[];Object.keys(paths).sort().forEach(function(path){{var item=paths[path]||{{}};Object.keys(item).forEach(function(method){{entries.push([method.toUpperCase(),path,(item[method]&&item[method].summary)||'']);}});}});if(entries.length===0){{list.textContent='No routes found.';routes.hidden=false;return;}}list.innerHTML='';entries.forEach(function(entry){{var row=document.createElement('div');row.className='route';row.innerHTML='<span class=\"method\">'+entry[0]+'</span> <code>'+entry[1]+'</code>'+(entry[2]?' <span class=\"muted\">'+entry[2]+'</span>':'');list.appendChild(row);}});routes.hidden=false;}}).catch(function(err){{status.textContent='Failed to load OpenAPI: '+err.message;}});}})();</script></body></html>"
    )
}
