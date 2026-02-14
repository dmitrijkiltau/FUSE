use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path};
use std::rc::Rc;

use fuse_rt::{
    bytes as rt_bytes, config as rt_config, error as rt_error, json as rt_json,
    validate as rt_validate,
};

use crate::ast::{
    Expr, ExprKind, HttpVerb, Ident, Literal, Pattern, PatternKind, TypeRef, TypeRefKind, UnaryOp,
};
use crate::db::Db;
use crate::html_tags::{self, HtmlTagKind};
use crate::interp::{Task, TaskResult, Value, format_error_value};
use crate::ir::{CallKind, Const, Function, Instr, Program as IrProgram, Service, ServiceRoute};
use crate::span::Span;

#[derive(Debug)]
enum VmError {
    Runtime(String),
    Error(Value),
}

type VmResult<T> = Result<T, VmError>;
type ServeHook<'a> = Box<dyn FnMut(i64) -> Result<Value, String> + 'a>;

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

fn min_log_level() -> LogLevel {
    std::env::var("FUSE_LOG")
        .ok()
        .and_then(|raw| parse_log_level(&raw))
        .unwrap_or(LogLevel::Info)
}

pub struct Vm<'a> {
    program: &'a IrProgram,
    configs: HashMap<String, HashMap<String, Value>>,
    db: Option<Db>,
    serve_hook: Option<ServeHook<'a>>,
}

impl<'a> Vm<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self {
            program,
            configs: HashMap::new(),
            db: None,
            serve_hook: None,
        }
    }

    pub fn set_serve_hook(&mut self, hook: Option<ServeHook<'a>>) {
        self.serve_hook = hook;
    }

    pub fn run_app(&mut self, name: Option<&str>) -> Result<(), String> {
        if let Err(err) = self.eval_configs() {
            return Err(self.render_error(err));
        }
        let app = if let Some(name) = name {
            self.program
                .apps
                .get(name)
                .ok_or_else(|| format!("app not found: {name}"))?
        } else {
            self.program
                .apps
                .values()
                .next()
                .ok_or_else(|| "no app found".to_string())?
        };
        if let Err(err) = self.exec_function(app, Vec::new()) {
            return Err(self.render_error(err));
        }
        Ok(())
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let func = self
            .program
            .functions
            .get(name)
            .ok_or_else(|| format!("unknown function {name}"))?;
        let value = match self.exec_function(func, args) {
            Ok(val) => self
                .wrap_function_result(func, val)
                .map_err(|err| self.render_error(err))?,
            Err(VmError::Error(err_val)) => {
                if self.is_result_type(func.ret.as_ref()) {
                    Value::ResultErr(Box::new(err_val))
                } else {
                    return Err(self.render_error(VmError::Error(err_val)));
                }
            }
            Err(VmError::Runtime(msg)) => return Err(msg),
        };
        Ok(value)
    }

    fn render_error(&self, err: VmError) -> String {
        match err {
            VmError::Runtime(msg) => msg,
            VmError::Error(value) => format_error_value(&value),
        }
    }

    fn eval_configs(&mut self) -> VmResult<()> {
        let config_path =
            std::env::var("FUSE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        let file_values = rt_config::load_config_file(&config_path).map_err(VmError::Runtime)?;
        for config in self.program.configs.values() {
            let mut map = HashMap::new();
            let section = file_values.get(&config.name);
            for field in &config.fields {
                let key = rt_config::env_key(&config.name, &field.name);
                let path = format!("{}.{}", config.name, field.name);
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
                            if let Some(raw) = section.get(&field.name) {
                                self.parse_env_value(&field.ty, raw)
                                    .map_err(|err| self.map_parse_error(err, &path))?
                            } else if let Some(fn_name) = &field.default_fn {
                                let func =
                                    self.program.functions.get(fn_name).ok_or_else(|| {
                                        VmError::Runtime(format!(
                                            "unknown config default {fn_name}"
                                        ))
                                    })?;
                                self.exec_function(func, Vec::new())?
                            } else {
                                Value::Null
                            }
                        } else if let Some(fn_name) = &field.default_fn {
                            let func = self.program.functions.get(fn_name).ok_or_else(|| {
                                VmError::Runtime(format!("unknown config default {fn_name}"))
                            })?;
                            self.exec_function(func, Vec::new())?
                        } else {
                            Value::Null
                        };
                        self.validate_value(&value, &field.ty, &path)?;
                        value
                    }
                };
                map.insert(field.name.clone(), value);
            }
            self.configs.insert(config.name.clone(), map);
        }
        Ok(())
    }

    fn map_parse_error(&self, err: VmError, path: &str) -> VmError {
        match err {
            VmError::Runtime(message) => {
                VmError::Error(self.validation_error_value(path, "invalid_value", message))
            }
            other => other,
        }
    }

    fn exec_function(&mut self, func: &Function, args: Vec<Value>) -> VmResult<Value> {
        let mut frame = Frame::new(func);
        for (idx, arg) in args.into_iter().enumerate() {
            if idx < frame.locals.len() {
                frame.locals[idx] = arg;
            }
        }
        loop {
            let instr = match frame.code.get(frame.ip) {
                Some(instr) => instr.clone(),
                None => return Ok(self.wrap_function_result(func, Value::Unit)?),
            };
            frame.ip += 1;
            match instr {
                Instr::Push(constant) => frame.stack.push(self.value_from_const(constant)),
                Instr::LoadLocal(slot) => {
                    let value = frame.locals.get(slot).cloned().unwrap_or(Value::Unit);
                    frame.stack.push(value);
                }
                Instr::StoreLocal(slot) => {
                    let value = frame.pop()?;
                    if slot >= frame.locals.len() {
                        return Err(VmError::Runtime(format!("invalid local slot {slot}")));
                    }
                    if let Value::Boxed(cell) = &frame.locals[slot] {
                        *cell.borrow_mut() = value.unboxed();
                    } else {
                        frame.locals[slot] = value;
                    }
                }
                Instr::Pop => {
                    frame.pop()?;
                }
                Instr::Dup => {
                    let value = frame.peek()?.clone();
                    frame.stack.push(value);
                }
                Instr::Neg => {
                    let value = frame.pop()?;
                    let value = match value {
                        Value::Int(v) => Value::Int(-v),
                        Value::Float(v) => Value::Float(-v),
                        _ => return Err(VmError::Runtime("unary '-' expects number".to_string())),
                    };
                    frame.stack.push(value);
                }
                Instr::Not => {
                    let value = frame.pop()?;
                    let value = match value {
                        Value::Bool(v) => Value::Bool(!v),
                        _ => return Err(VmError::Runtime("unary 'not' expects bool".to_string())),
                    };
                    frame.stack.push(value);
                }
                Instr::Add => {
                    let right = frame.pop()?;
                    let left = frame.pop()?;
                    frame.stack.push(self.eval_add(left, right)?);
                }
                Instr::Sub | Instr::Mul | Instr::Div | Instr::Mod => {
                    let right = frame.pop()?;
                    let left = frame.pop()?;
                    frame.stack.push(self.eval_arith(&instr, left, right)?);
                }
                Instr::Eq | Instr::NotEq | Instr::Lt | Instr::LtEq | Instr::Gt | Instr::GtEq => {
                    let right = frame.pop()?;
                    let left = frame.pop()?;
                    frame.stack.push(self.eval_compare(&instr, left, right)?);
                }
                Instr::And | Instr::Or => {
                    let right = frame.pop()?;
                    let left = frame.pop()?;
                    frame.stack.push(self.eval_bool(&instr, left, right)?);
                }
                Instr::Jump(target) => {
                    frame.ip = target;
                }
                Instr::JumpIfFalse(target) => {
                    let value = frame.pop()?;
                    let cond = self.as_bool(&value)?;
                    if !cond {
                        frame.ip = target;
                    }
                }
                Instr::JumpIfNull(target) => {
                    let value = frame.pop()?;
                    if matches!(value.unboxed(), Value::Null) {
                        frame.ip = target;
                    }
                }
                Instr::Call { name, argc, kind } => {
                    let mut args = Vec::new();
                    for _ in 0..argc {
                        args.push(frame.pop()?);
                    }
                    args.reverse();
                    let value = match kind {
                        CallKind::Builtin => self.eval_builtin(&name, args)?,
                        CallKind::Function => {
                            let func = self.program.functions.get(&name).ok_or_else(|| {
                                VmError::Runtime(format!("unknown function {name}"))
                            })?;
                            match self.exec_function(func, args) {
                                Ok(val) => self.wrap_function_result(func, val)?,
                                Err(VmError::Error(err_val)) => {
                                    if self.is_result_type(func.ret.as_ref()) {
                                        Value::ResultErr(Box::new(err_val))
                                    } else {
                                        return Err(VmError::Error(err_val));
                                    }
                                }
                                Err(VmError::Runtime(msg)) => return Err(VmError::Runtime(msg)),
                            }
                        }
                    };
                    frame.stack.push(value);
                }
                Instr::Spawn { name, argc } => {
                    let mut args = Vec::new();
                    for _ in 0..argc {
                        args.push(frame.pop()?);
                    }
                    args.reverse();
                    let func = self
                        .program
                        .functions
                        .get(&name)
                        .ok_or_else(|| VmError::Runtime(format!("unknown function {name}")))?;
                    let result = match self.exec_function(func, args) {
                        Ok(val) => {
                            let value = self.wrap_function_result(func, val)?;
                            TaskResult::Ok(value)
                        }
                        Err(VmError::Error(err_val)) => TaskResult::Error(err_val),
                        Err(VmError::Runtime(msg)) => TaskResult::Runtime(msg),
                    };
                    frame
                        .stack
                        .push(Value::Task(Task::from_task_result(result)));
                }
                Instr::Await => {
                    let value = frame.pop()?;
                    match value {
                        Value::Task(task) => match task.result_raw() {
                            TaskResult::Ok(value) => frame.stack.push(value),
                            TaskResult::Error(err) => return Err(VmError::Error(err)),
                            TaskResult::Runtime(msg) => return Err(VmError::Runtime(msg)),
                        },
                        _ => {
                            return Err(VmError::Runtime("await expects a Task value".to_string()));
                        }
                    }
                }
                Instr::Return => {
                    let value = if frame.stack.is_empty() {
                        Value::Unit
                    } else {
                        frame.pop()?
                    };
                    return Ok(self.wrap_function_result(func, value)?);
                }
                Instr::Bang { has_error } => {
                    let err_value = if has_error { Some(frame.pop()?) } else { None };
                    let value = frame.pop()?.unboxed();
                    match value {
                        Value::Null => {
                            let err = err_value
                                .unwrap_or_else(|| self.default_error_value("missing value"));
                            return Err(VmError::Error(err));
                        }
                        Value::ResultOk(ok) => {
                            frame.stack.push(*ok);
                        }
                        Value::ResultErr(err) => {
                            let err = err_value.unwrap_or(*err);
                            return Err(VmError::Error(err));
                        }
                        other => {
                            frame.stack.push(other);
                        }
                    }
                }
                Instr::MakeList { len } => {
                    let mut items = Vec::with_capacity(len);
                    for _ in 0..len {
                        items.push(frame.pop()?);
                    }
                    items.reverse();
                    frame.stack.push(Value::List(items));
                }
                Instr::MakeMap { len } => {
                    let mut pairs = Vec::with_capacity(len);
                    for _ in 0..len {
                        let value = frame.pop()?;
                        let key = frame.pop()?;
                        pairs.push((key, value));
                    }
                    pairs.reverse();
                    let mut map = HashMap::new();
                    for (key_val, value) in pairs {
                        let key = match &key_val {
                            Value::String(text) => text.clone(),
                            _ => {
                                return Err(VmError::Runtime(format!(
                                    "map keys must be strings, got {}",
                                    self.value_type_name(&key_val)
                                )));
                            }
                        };
                        map.insert(key, value);
                    }
                    frame.stack.push(Value::Map(map));
                }
                Instr::MakeBox => {
                    let value = frame.pop()?;
                    let boxed = match value {
                        Value::Boxed(_) => value,
                        other => Value::Boxed(Rc::new(RefCell::new(other))),
                    };
                    frame.stack.push(boxed);
                }
                Instr::MakeStruct { name, fields } => {
                    let mut values = HashMap::new();
                    for field in fields.into_iter().rev() {
                        let value = frame.pop()?;
                        values.insert(field, value);
                    }
                    let value = self.make_struct(&name, values)?;
                    frame.stack.push(value);
                }
                Instr::MakeEnum {
                    name,
                    variant,
                    argc,
                } => {
                    let mut payload = Vec::with_capacity(argc);
                    for _ in 0..argc {
                        payload.push(frame.pop()?);
                    }
                    payload.reverse();
                    let value = self.make_enum(&name, &variant, payload)?;
                    frame.stack.push(value);
                }
                Instr::GetField { field } => {
                    let base = frame.pop()?.unboxed();
                    let value = match base {
                        Value::Struct { fields, .. } => fields
                            .get(&field)
                            .cloned()
                            .ok_or_else(|| VmError::Runtime(format!("unknown field {field}")))?,
                        Value::Config(name) => {
                            let map = self.configs.get(&name).ok_or_else(|| {
                                VmError::Runtime(format!("unknown config {name}"))
                            })?;
                            map.get(&field).cloned().ok_or_else(|| {
                                VmError::Runtime(format!("unknown config field {name}.{field}"))
                            })?
                        }
                        _ => {
                            return Err(VmError::Runtime(
                                "member access not supported on this value".to_string(),
                            ));
                        }
                    };
                    frame.stack.push(value);
                }
                Instr::GetIndex => {
                    let index = frame.pop()?;
                    let base = frame.pop()?.unboxed();
                    let value = match base {
                        Value::List(items) => {
                            let idx = self.list_index(&index)?;
                            items.get(idx).cloned().ok_or_else(|| {
                                VmError::Runtime("index out of bounds".to_string())
                            })?
                        }
                        Value::Map(items) => {
                            let key = self.map_key(&index)?;
                            items.get(&key).cloned().unwrap_or(Value::Null)
                        }
                        Value::Null => {
                            return Err(VmError::Runtime("null access".to_string()));
                        }
                        _ => {
                            return Err(VmError::Runtime(
                                "index access not supported on this value".to_string(),
                            ));
                        }
                    };
                    frame.stack.push(value);
                }
                Instr::SetField { field } => {
                    let value = frame.pop()?;
                    let base = frame.pop()?;
                    let field_name = field;
                    let updated = match base {
                        Value::Boxed(cell) => {
                            {
                                let mut inner = cell.borrow_mut();
                                match &mut *inner {
                                    Value::Struct { fields, .. } => {
                                        fields.insert(field_name.clone(), value);
                                    }
                                    _ => {
                                        return Err(VmError::Runtime(
                                            "assignment target must be a struct field".to_string(),
                                        ));
                                    }
                                }
                            }
                            Value::Boxed(cell)
                        }
                        Value::Struct { name, mut fields } => {
                            fields.insert(field_name, value);
                            Value::Struct { name, fields }
                        }
                        _ => {
                            return Err(VmError::Runtime(
                                "assignment target must be a struct field".to_string(),
                            ));
                        }
                    };
                    frame.stack.push(updated);
                }
                Instr::SetIndex => {
                    let value = frame.pop()?;
                    let index = frame.pop()?;
                    let base = frame.pop()?;
                    let updated = match base {
                        Value::Boxed(cell) => {
                            {
                                let mut inner = cell.borrow_mut();
                                match &mut *inner {
                                    Value::List(items) => {
                                        let idx = self.list_index(&index)?;
                                        if idx >= items.len() {
                                            return Err(VmError::Runtime(
                                                "index out of bounds".to_string(),
                                            ));
                                        }
                                        items[idx] = value;
                                    }
                                    Value::Map(items) => {
                                        let key = self.map_key(&index)?;
                                        items.insert(key, value);
                                    }
                                    Value::Null => {
                                        return Err(VmError::Runtime("null access".to_string()));
                                    }
                                    _ => {
                                        return Err(VmError::Runtime(
                                            "assignment target must be an indexable value"
                                                .to_string(),
                                        ));
                                    }
                                }
                            }
                            Value::Boxed(cell)
                        }
                        Value::List(mut items) => {
                            let idx = self.list_index(&index)?;
                            if idx >= items.len() {
                                return Err(VmError::Runtime("index out of bounds".to_string()));
                            }
                            items[idx] = value;
                            Value::List(items)
                        }
                        Value::Map(mut items) => {
                            let key = self.map_key(&index)?;
                            items.insert(key, value);
                            Value::Map(items)
                        }
                        Value::Null => return Err(VmError::Runtime("null access".to_string())),
                        _ => {
                            return Err(VmError::Runtime(
                                "assignment target must be an indexable value".to_string(),
                            ));
                        }
                    };
                    frame.stack.push(updated);
                }
                Instr::InterpString { parts } => {
                    let mut items = Vec::with_capacity(parts);
                    for _ in 0..parts {
                        items.push(frame.pop()?);
                    }
                    items.reverse();
                    let mut out = String::new();
                    for part in items {
                        out.push_str(&part.to_string_value());
                    }
                    frame.stack.push(Value::String(out));
                }
                Instr::MatchLocal {
                    slot,
                    pat,
                    bindings,
                    jump,
                } => {
                    let value = frame.locals.get(slot).cloned().unwrap_or(Value::Unit);
                    let mut bound = HashMap::new();
                    if self.match_pattern(&value, &pat, &mut bound)? {
                        for (name, slot) in bindings {
                            if let Some(val) = bound.get(&name) {
                                if slot < frame.locals.len() {
                                    frame.locals[slot] = val.clone();
                                } else {
                                    return Err(VmError::Runtime(format!(
                                        "invalid binding slot {slot}"
                                    )));
                                }
                            }
                        }
                    } else {
                        frame.ip = jump;
                    }
                }
                Instr::LoadConfigField { config, field } => {
                    let map = self
                        .configs
                        .get(&config)
                        .ok_or_else(|| VmError::Runtime(format!("unknown config {config}")))?;
                    let value = map.get(&field).cloned().ok_or_else(|| {
                        VmError::Runtime(format!("unknown config field {config}.{field}"))
                    })?;
                    frame.stack.push(value);
                }
                Instr::IterInit => {
                    let value = frame.pop()?.unboxed();
                    let iter_values = match value {
                        Value::List(items) => items,
                        Value::Map(items) => items.into_values().collect(),
                        other => {
                            return Err(VmError::Runtime(format!(
                                "cannot iterate over {}",
                                self.value_type_name(&other)
                            )));
                        }
                    };
                    frame
                        .stack
                        .push(Value::Iterator(crate::interp::IteratorValue::new(
                            iter_values,
                        )));
                }
                Instr::IterNext { jump } => {
                    let iter_value = frame.pop()?.unboxed();
                    let mut iter = match iter_value {
                        Value::Iterator(iter) => iter,
                        other => {
                            return Err(VmError::Runtime(format!(
                                "expected iterator, got {}",
                                self.value_type_name(&other)
                            )));
                        }
                    };
                    if iter.index >= iter.values.len() {
                        frame.ip = jump;
                    } else {
                        let item = iter.values[iter.index].clone();
                        iter.index += 1;
                        frame.stack.push(item);
                        frame.stack.push(Value::Iterator(iter));
                    }
                }
                Instr::RuntimeError(message) => {
                    return Err(VmError::Runtime(message));
                }
            }
        }
    }

    fn db_url(&self) -> VmResult<String> {
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
        Err(VmError::Runtime(
            "db url not configured (set FUSE_DB_URL or App.dbUrl)".to_string(),
        ))
    }

    fn db_mut(&mut self) -> VmResult<&mut Db> {
        if self.db.is_none() {
            let url = self.db_url()?;
            let db = Db::open(&url).map_err(VmError::Runtime)?;
            self.db = Some(db);
        }
        Ok(self.db.as_mut().expect("db initialized"))
    }

    fn eval_builtin(&mut self, name: &str, args: Vec<Value>) -> VmResult<Value> {
        let args: Vec<Value> = args.into_iter().map(|val| val.unboxed()).collect();
        if html_tags::is_html_tag(name) {
            return self.eval_html_tag_builtin(name, &args);
        }
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
                    .ok_or_else(|| VmError::Runtime("json.encode expects a value".to_string()))?;
                let json = self.value_to_json(&value);
                Ok(Value::String(rt_json::encode(&json)))
            }
            "json.decode" => {
                let text = match args.get(0) {
                    Some(Value::String(text)) => text.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "json.decode expects a string argument".to_string(),
                        ));
                    }
                };
                let json = rt_json::decode(&text)
                    .map_err(|msg| VmError::Runtime(format!("invalid json: {msg}")))?;
                Ok(self.json_to_value(&json))
            }
            "asset" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("asset expects 1 argument".to_string()));
                }
                let path = match args.get(0) {
                    Some(Value::String(path)) => path,
                    _ => return Err(VmError::Runtime("asset expects a string path".to_string())),
                };
                Ok(Value::String(crate::runtime_assets::resolve_asset_href(
                    path,
                )))
            }
            "html.text" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("html.text expects 1 argument".to_string()));
                }
                let text = match args.get(0) {
                    Some(Value::String(text)) => text.clone(),
                    _ => return Err(VmError::Runtime("html.text expects a String".to_string())),
                };
                Ok(Value::Html(crate::interp::HtmlNode::Text(text)))
            }
            "html.raw" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("html.raw expects 1 argument".to_string()));
                }
                let text = match args.get(0) {
                    Some(Value::String(text)) => text.clone(),
                    _ => return Err(VmError::Runtime("html.raw expects a String".to_string())),
                };
                Ok(Value::Html(crate::interp::HtmlNode::Raw(text)))
            }
            "html.node" => {
                if args.len() != 3 {
                    return Err(VmError::Runtime(
                        "html.node expects 3 arguments".to_string(),
                    ));
                }
                let tag = match args.get(0) {
                    Some(Value::String(tag)) => tag.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "html.node expects a String tag".to_string(),
                        ));
                    }
                };
                let attrs = match args.get(1) {
                    Some(Value::Map(map)) => {
                        let mut attrs = HashMap::with_capacity(map.len());
                        for (key, value) in map {
                            let Value::String(text) = value else {
                                return Err(VmError::Runtime(
                                    "html.node attrs must be Map<String, String>".to_string(),
                                ));
                            };
                            attrs.insert(key.clone(), text.clone());
                        }
                        attrs
                    }
                    _ => {
                        return Err(VmError::Runtime(
                            "html.node expects attrs as Map<String, String>".to_string(),
                        ));
                    }
                };
                let children = match args.get(2) {
                    Some(Value::List(items)) => {
                        let mut children = Vec::with_capacity(items.len());
                        for item in items {
                            let Value::Html(node) = item else {
                                return Err(VmError::Runtime(
                                    "html.node children must be List<Html>".to_string(),
                                ));
                            };
                            children.push(node.clone());
                        }
                        children
                    }
                    _ => {
                        return Err(VmError::Runtime(
                            "html.node expects children as List<Html>".to_string(),
                        ));
                    }
                };
                Ok(Value::Html(crate::interp::HtmlNode::Element {
                    tag,
                    attrs,
                    children,
                }))
            }
            "html.render" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime(
                        "html.render expects 1 argument".to_string(),
                    ));
                }
                let node = match args.get(0) {
                    Some(Value::Html(node)) => node,
                    _ => {
                        return Err(VmError::Runtime(
                            "html.render expects an Html value".to_string(),
                        ));
                    }
                };
                Ok(Value::String(node.render_to_string()))
            }
            "svg.inline" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime(
                        "svg.inline expects 1 argument".to_string(),
                    ));
                }
                let name = match args.get(0) {
                    Some(Value::String(name)) => name,
                    _ => {
                        return Err(VmError::Runtime(
                            "svg.inline expects a String path".to_string(),
                        ));
                    }
                };
                let svg = crate::runtime_svg::load_svg_inline(name).map_err(VmError::Runtime)?;
                Ok(Value::Html(crate::interp::HtmlNode::Raw(svg)))
            }
            "db.exec" => {
                if args.len() > 2 {
                    return Err(VmError::Runtime(
                        "db.exec expects 1 or 2 arguments".to_string(),
                    ));
                }
                let sql = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => return Err(VmError::Runtime("db.exec expects a SQL string".to_string())),
                };
                let params = if args.len() > 1 {
                    match args.get(1) {
                        Some(Value::List(items)) => items.clone(),
                        _ => {
                            return Err(VmError::Runtime(
                                "db.exec params must be a list".to_string(),
                            ));
                        }
                    }
                } else {
                    Vec::new()
                };
                let db = self.db_mut()?;
                db.exec_params(&sql, &params).map_err(VmError::Runtime)?;
                Ok(Value::Unit)
            }
            "db.query" => {
                if args.len() > 2 {
                    return Err(VmError::Runtime(
                        "db.query expects 1 or 2 arguments".to_string(),
                    ));
                }
                let sql = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "db.query expects a SQL string".to_string(),
                        ));
                    }
                };
                let params = if args.len() > 1 {
                    match args.get(1) {
                        Some(Value::List(items)) => items.clone(),
                        _ => {
                            return Err(VmError::Runtime(
                                "db.query params must be a list".to_string(),
                            ));
                        }
                    }
                } else {
                    Vec::new()
                };
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(VmError::Runtime)?;
                let list = rows.into_iter().map(Value::Map).collect();
                Ok(Value::List(list))
            }
            "db.one" => {
                if args.len() > 2 {
                    return Err(VmError::Runtime(
                        "db.one expects 1 or 2 arguments".to_string(),
                    ));
                }
                let sql = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => return Err(VmError::Runtime("db.one expects a SQL string".to_string())),
                };
                let params = if args.len() > 1 {
                    match args.get(1) {
                        Some(Value::List(items)) => items.clone(),
                        _ => {
                            return Err(VmError::Runtime(
                                "db.one params must be a list".to_string(),
                            ));
                        }
                    }
                } else {
                    Vec::new()
                };
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(VmError::Runtime)?;
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
                        return Err(VmError::Runtime(
                            "db.from expects a table name string".to_string(),
                        ));
                    }
                };
                let query = crate::db::Query::new(table).map_err(VmError::Runtime)?;
                Ok(Value::Query(query))
            }
            "query.select" => {
                if args.len() != 2 {
                    return Err(VmError::Runtime(
                        "query.select expects 2 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.select expects a Query".to_string())),
                };
                let columns = match args.get(1) {
                    Some(Value::List(items)) => {
                        let mut out = Vec::with_capacity(items.len());
                        for item in items {
                            match item {
                                Value::String(text) => out.push(text.clone()),
                                _ => {
                                    return Err(VmError::Runtime(
                                        "query.select expects a list of strings".to_string(),
                                    ));
                                }
                            }
                        }
                        out
                    }
                    _ => {
                        return Err(VmError::Runtime(
                            "query.select expects a list of strings".to_string(),
                        ));
                    }
                };
                let next = query.select(columns).map_err(VmError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.where" => {
                if args.len() != 4 {
                    return Err(VmError::Runtime(
                        "query.where expects 4 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.where expects a Query".to_string())),
                };
                let column = match args.get(1) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "query.where expects a column string".to_string(),
                        ));
                    }
                };
                let op = match args.get(2) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "query.where expects an operator string".to_string(),
                        ));
                    }
                };
                let value = args
                    .get(3)
                    .cloned()
                    .ok_or_else(|| VmError::Runtime("query.where expects a value".to_string()))?;
                let next = query
                    .where_clause(column, op, value)
                    .map_err(VmError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.order_by" => {
                if args.len() != 3 {
                    return Err(VmError::Runtime(
                        "query.order_by expects 3 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "query.order_by expects a Query".to_string(),
                        ));
                    }
                };
                let column = match args.get(1) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "query.order_by expects a column string".to_string(),
                        ));
                    }
                };
                let dir = match args.get(2) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(VmError::Runtime(
                            "query.order_by expects a direction string".to_string(),
                        ));
                    }
                };
                let next = query.order_by(column, dir).map_err(VmError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.limit" => {
                if args.len() != 2 {
                    return Err(VmError::Runtime(
                        "query.limit expects 2 arguments".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.limit expects a Query".to_string())),
                };
                let limit = match args.get(1) {
                    Some(Value::Int(v)) => *v,
                    Some(Value::Float(v)) => *v as i64,
                    _ => return Err(VmError::Runtime("query.limit expects an Int".to_string())),
                };
                let next = query.limit(limit).map_err(VmError::Runtime)?;
                Ok(Value::Query(next))
            }
            "query.one" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("query.one expects 1 argument".to_string()));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.one expects a Query".to_string())),
                };
                let (sql, params) = query.build_sql(Some(1)).map_err(VmError::Runtime)?;
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(VmError::Runtime)?;
                if let Some(row) = rows.into_iter().next() {
                    Ok(Value::Map(row))
                } else {
                    Ok(Value::Null)
                }
            }
            "query.all" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("query.all expects 1 argument".to_string()));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.all expects a Query".to_string())),
                };
                let (sql, params) = query.build_sql(None).map_err(VmError::Runtime)?;
                let db = self.db_mut()?;
                let rows = db.query_params(&sql, &params).map_err(VmError::Runtime)?;
                let list = rows.into_iter().map(Value::Map).collect();
                Ok(Value::List(list))
            }
            "query.exec" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime(
                        "query.exec expects 1 argument".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.exec expects a Query".to_string())),
                };
                let (sql, params) = query.build_sql(None).map_err(VmError::Runtime)?;
                let db = self.db_mut()?;
                db.exec_params(&sql, &params).map_err(VmError::Runtime)?;
                Ok(Value::Unit)
            }
            "query.sql" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("query.sql expects 1 argument".to_string()));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.sql expects a Query".to_string())),
                };
                let sql = query.sql().map_err(VmError::Runtime)?;
                Ok(Value::String(sql))
            }
            "query.params" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime(
                        "query.params expects 1 argument".to_string(),
                    ));
                }
                let query = match args.get(0) {
                    Some(Value::Query(query)) => query.clone(),
                    _ => return Err(VmError::Runtime("query.params expects a Query".to_string())),
                };
                let params = query.params().map_err(VmError::Runtime)?;
                Ok(Value::List(params))
            }
            "task.id" => match args.get(0) {
                Some(Value::Task(task)) => Ok(Value::String(format!("task-{}", task.id()))),
                _ => Err(VmError::Runtime(
                    "task.id expects a Task argument".to_string(),
                )),
            },
            "task.done" => match args.get(0) {
                Some(Value::Task(task)) => Ok(Value::Bool(task.is_done())),
                _ => Err(VmError::Runtime(
                    "task.done expects a Task argument".to_string(),
                )),
            },
            "task.cancel" => match args.get(0) {
                Some(Value::Task(task)) => Ok(Value::Bool(task.cancel())),
                _ => Err(VmError::Runtime(
                    "task.cancel expects a Task argument".to_string(),
                )),
            },
            "assert" => {
                let cond = match args.get(0) {
                    Some(Value::Bool(value)) => *value,
                    _ => {
                        return Err(VmError::Runtime(
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
                Err(VmError::Runtime(format!("assert failed: {message}")))
            }
            "range" => {
                if args.len() != 2 {
                    return Err(VmError::Runtime("range expects 2 arguments".to_string()));
                }
                self.eval_range(&args[0], &args[1])
            }
            "env" => {
                let key = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => {
                        return Err(VmError::Runtime(
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
                    _ => return Err(VmError::Runtime("serve expects a port number".to_string())),
                };
                if let Some(hook) = self.serve_hook.as_mut() {
                    hook(port).map_err(VmError::Runtime)
                } else {
                    self.eval_serve(port)
                }
            }
            _ => Err(VmError::Runtime(format!("unknown builtin {name}"))),
        }
    }

    fn eval_html_tag_builtin(&self, name: &str, args: &[Value]) -> VmResult<Value> {
        let Some(kind) = html_tags::tag_kind(name) else {
            return Err(VmError::Runtime(format!("unknown builtin {name}")));
        };
        let max = match kind {
            HtmlTagKind::Normal => 2usize,
            HtmlTagKind::Void => 1usize,
        };
        if args.len() > max {
            return Err(VmError::Runtime(format!(
                "{} expects at most {} arguments",
                name, max
            )));
        }
        let attrs = match args.get(0) {
            Some(Value::Map(map)) => {
                let mut attrs = HashMap::with_capacity(map.len());
                for (key, value) in map {
                    let Value::String(text) = value else {
                        return Err(VmError::Runtime(format!(
                            "{} attrs must be Map<String, String>",
                            name
                        )));
                    };
                    attrs.insert(key.clone(), text.clone());
                }
                attrs
            }
            Some(_) => {
                return Err(VmError::Runtime(format!(
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
                            return Err(VmError::Runtime(format!(
                                "{} children must be List<Html>",
                                name
                            )));
                        };
                        children.push(node.clone());
                    }
                    children
                }
                Some(_) => {
                    return Err(VmError::Runtime(format!(
                        "{} expects children as List<Html>",
                        name
                    )));
                }
                None => Vec::new(),
            },
        };
        Ok(Value::Html(crate::interp::HtmlNode::Element {
            tag: name.to_string(),
            attrs,
            children,
        }))
    }

    fn eval_range(&self, start: &Value, end: &Value) -> VmResult<Value> {
        match (start.unboxed(), end.unboxed()) {
            (Value::Int(start), Value::Int(end)) => {
                if start > end {
                    return Err(VmError::Runtime("range start must be <= end".to_string()));
                }
                let items = (start..=end).map(Value::Int).collect();
                Ok(Value::List(items))
            }
            (Value::Float(start), Value::Float(end)) => self.eval_float_range(start, end),
            (Value::Int(start), Value::Float(end)) => self.eval_float_range(start as f64, end),
            (Value::Float(start), Value::Int(end)) => self.eval_float_range(start, end as f64),
            _ => Err(VmError::Runtime("range expects numeric bounds".to_string())),
        }
    }

    fn eval_float_range(&self, start: f64, end: f64) -> VmResult<Value> {
        if !start.is_finite() || !end.is_finite() {
            return Err(VmError::Runtime("invalid range bounds".to_string()));
        }
        if start > end {
            return Err(VmError::Runtime("range start must be <= end".to_string()));
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

    fn eval_serve(&mut self, port: i64) -> VmResult<Value> {
        let service = self.select_service()?.clone();
        let host = std::env::var("FUSE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = port
            .try_into()
            .map_err(|_| VmError::Runtime("invalid port".to_string()))?;
        let addr = format!("{host}:{port}");
        let listener = TcpListener::bind(&addr)
            .map_err(|err| VmError::Runtime(format!("failed to bind {addr}: {err}")))?;
        let max_requests = std::env::var("FUSE_MAX_REQUESTS")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(0);
        let mut handled = 0usize;
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    return Err(VmError::Runtime(format!(
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
        Ok(Value::Unit)
    }

    fn select_service(&self) -> VmResult<&Service> {
        if self.program.services.is_empty() {
            return Err(VmError::Runtime("no service declared".to_string()));
        }
        if let Ok(name) = std::env::var("FUSE_SERVICE") {
            return self
                .program
                .services
                .get(&name)
                .ok_or_else(|| VmError::Runtime(format!("service not found: {name}")));
        }
        if self.program.services.len() == 1 {
            return Ok(self.program.services.values().next().unwrap());
        }
        Err(VmError::Runtime(
            "multiple services declared; set FUSE_SERVICE".to_string(),
        ))
    }

    fn handle_http_request(
        &mut self,
        service: &Service,
        stream: &mut TcpStream,
    ) -> VmResult<String> {
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
        if let Some(response) = self.try_openapi_ui_response(request.method.as_str(), &path) {
            return Ok(response);
        }
        if let Some(response) = self.try_static_response(request.method.as_str(), &path) {
            return Ok(response);
        }
        let (route, params) = match self.match_route(service, &verb, &path)? {
            Some(result) => result,
            None => {
                if let Some(response) = self.try_vite_proxy_response(&request) {
                    return Ok(response);
                }
                let body = self.error_json_from_code("not_found", "not found");
                return Ok(self.http_response(404, body));
            }
        };
        let body_value = if let Some(body_ty) = &route.body_type {
            let body_text = String::from_utf8_lossy(&request.body);
            if body_text.trim().is_empty() {
                return Err(VmError::Error(self.validation_error_value(
                    "body",
                    "missing_field",
                    "missing JSON body",
                )));
            }
            let json = rt_json::decode(&body_text).map_err(|msg| {
                VmError::Error(self.validation_error_value("body", "invalid_json", msg))
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
                    let body =
                        self.maybe_inject_live_reload_html(self.render_html_value(ok.as_ref())?);
                    Ok(self.http_response_with_type(200, body, "text/html; charset=utf-8"))
                } else {
                    let json = self.value_to_json(&ok);
                    Ok(self.http_response(200, rt_json::encode(&json)))
                }
            }
            other => {
                if html_response {
                    let body = self.maybe_inject_live_reload_html(self.render_html_value(&other)?);
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
        route: &ServiceRoute,
        mut params: Vec<Value>,
        body_value: Option<Value>,
    ) -> VmResult<Value> {
        if let Some(body) = body_value {
            params.push(body);
        }
        let func =
            self.program.functions.get(&route.handler).ok_or_else(|| {
                VmError::Runtime(format!("unknown route handler {}", route.handler))
            })?;
        self.exec_function(func, params)
    }

    fn match_route<'r>(
        &mut self,
        service: &'r Service,
        verb: &HttpVerb,
        path: &str,
    ) -> VmResult<Option<(&'r ServiceRoute, Vec<Value>)>> {
        let base_segments = split_path(&service.base_path);
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
            let route_segments = split_path(&route.path);
            if route_segments.len() != req_segments.len() {
                continue;
            }
            let mut params = Vec::new();
            let mut matched = true;
            for (seg, req) in route_segments.iter().zip(req_segments.iter()) {
                if let Some((name, ty_name)) = parse_route_param(seg) {
                    let ty = TypeRef {
                        kind: TypeRefKind::Simple(Ident {
                            name: ty_name.to_string(),
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    };
                    let value = self
                        .parse_env_value(&ty, req)
                        .map_err(|err| self.map_parse_error(err, &name))?;
                    self.validate_value(&value, &ty, &name)?;
                    params.push(value);
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

    fn read_http_request(&self, stream: &mut TcpStream) -> VmResult<HttpRequest> {
        let mut buffer = Vec::new();
        let mut temp = [0u8; 1024];
        let mut header_end = None;
        loop {
            let read = stream
                .read(&mut temp)
                .map_err(|err| VmError::Runtime(format!("failed to read request: {err}")))?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..read]);
            if let Some(pos) = find_header_end(&buffer) {
                header_end = Some(pos);
                break;
            }
            if buffer.len() > 1024 * 1024 {
                return Err(VmError::Runtime("request header too large".to_string()));
            }
        }
        let header_end = header_end
            .ok_or_else(|| VmError::Runtime("invalid HTTP request: missing headers".to_string()))?;
        let header_bytes = &buffer[..header_end];
        let header_text = String::from_utf8_lossy(header_bytes);
        let mut lines = header_text.split("\r\n");
        let request_line = lines
            .next()
            .ok_or_else(|| VmError::Runtime("invalid HTTP request line".to_string()))?;
        let mut parts = request_line.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| VmError::Runtime("invalid HTTP request line".to_string()))?
            .to_string();
        let path = parts
            .next()
            .ok_or_else(|| VmError::Runtime("invalid HTTP request line".to_string()))?
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
                .map_err(|err| VmError::Runtime(format!("failed to read body: {err}")))?;
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

    fn http_error_response(&self, err: VmError) -> String {
        match err {
            VmError::Error(value) => {
                let status = self.http_status_for_error_value(&value);
                let body = self.error_json_from_value(&value);
                self.http_response(status, body)
            }
            VmError::Runtime(message) => {
                let body = self.internal_error_json(&message);
                self.http_response(500, body)
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

    fn render_html_value(&self, value: &Value) -> VmResult<String> {
        match value.unboxed() {
            Value::Html(node) => Ok(node.render_to_string()),
            other => Err(VmError::Runtime(format!(
                "expected Html response, got {}",
                self.value_type_name(&other)
            ))),
        }
    }

    fn value_from_const(&self, constant: Const) -> Value {
        match constant {
            Const::Unit => Value::Unit,
            Const::Int(v) => Value::Int(v),
            Const::Float(v) => Value::Float(v),
            Const::Bool(v) => Value::Bool(v),
            Const::String(v) => Value::String(v),
            Const::Null => Value::Null,
        }
    }

    fn parse_env_value(&self, ty: &TypeRef, raw: &str) -> VmResult<Value> {
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
            TypeRefKind::Result { .. } => Err(VmError::Runtime(
                "Result is not supported for config env overrides".to_string(),
            )),
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(VmError::Runtime(
                            "Option expects 1 type argument".to_string(),
                        ));
                    }
                    if raw.eq_ignore_ascii_case("null") || raw.is_empty() {
                        Ok(Value::Null)
                    } else {
                        self.parse_env_value(&args[0], raw)
                    }
                }
                "Result" => Err(VmError::Runtime(
                    "Result is not supported for config env overrides".to_string(),
                )),
                _ => Err(VmError::Runtime(
                    "config env overrides only support simple types".to_string(),
                )),
            },
        }
    }

    fn parse_simple_env(&self, name: &str, raw: &str) -> VmResult<Value> {
        match name {
            "Int" => raw
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| VmError::Runtime(format!("invalid Int: {raw}"))),
            "Float" => raw
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| VmError::Runtime(format!("invalid Float: {raw}"))),
            "Bool" => match raw.to_ascii_lowercase().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(VmError::Runtime(format!("invalid Bool: {raw}"))),
            },
            "String" | "Id" | "Email" => Ok(Value::String(raw.to_string())),
            "Bytes" => {
                let bytes = rt_bytes::decode_base64(raw)
                    .map_err(|msg| VmError::Runtime(format!("invalid Bytes (base64): {msg}")))?;
                Ok(Value::Bytes(bytes))
            }
            _ => Err(VmError::Runtime(format!(
                "env override not supported for type {name}"
            ))),
        }
    }

    fn validate_value(&self, value: &Value, ty: &TypeRef, path: &str) -> VmResult<()> {
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
                _ => Err(VmError::Error(self.validation_error_value(
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
                        return Err(VmError::Runtime(
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
                        return Err(VmError::Runtime(
                            "Result expects 2 type arguments".to_string(),
                        ));
                    }
                    match value {
                        Value::ResultOk(inner) => self.validate_value(&inner, &args[0], path),
                        Value::ResultErr(inner) => self.validate_value(&inner, &args[1], path),
                        _ => Err(VmError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Result, got {}", self.value_type_name(&value)),
                        ))),
                    }
                }
                "List" => {
                    if args.len() != 1 {
                        return Err(VmError::Runtime("List expects 1 type argument".to_string()));
                    }
                    match value {
                        Value::List(items) => {
                            for (idx, item) in items.iter().enumerate() {
                                let item_path = format!("{path}[{idx}]");
                                self.validate_value(item, &args[0], &item_path)?;
                            }
                            Ok(())
                        }
                        _ => Err(VmError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected List, got {}", self.value_type_name(&value)),
                        ))),
                    }
                }
                "Map" => {
                    if args.len() != 2 {
                        return Err(VmError::Runtime("Map expects 2 type arguments".to_string()));
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
                        _ => Err(VmError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Map, got {}", self.value_type_name(&value)),
                        ))),
                    }
                }
                _ => Err(VmError::Runtime(format!(
                    "validation not supported for {}",
                    base.name
                ))),
            },
        }
    }

    fn validate_simple(&self, value: &Value, name: &str, path: &str) -> VmResult<()> {
        let value = value.unboxed();
        let type_name = self.value_type_name(&value);
        let (module, simple_name) = split_type_name(name);
        if module.is_none() {
            match simple_name {
                "Int" => {
                    if matches!(value, Value::Int(_)) {
                        return Ok(());
                    }
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Int, got {type_name}"),
                    )));
                }
                "Float" => {
                    if matches!(value, Value::Float(_)) {
                        return Ok(());
                    }
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Float, got {type_name}"),
                    )));
                }
                "Bool" => {
                    if matches!(value, Value::Bool(_)) {
                        return Ok(());
                    }
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bool, got {type_name}"),
                    )));
                }
                "String" => {
                    if matches!(value, Value::String(_)) {
                        return Ok(());
                    }
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected String, got {type_name}"),
                    )));
                }
                "Id" => match value {
                    Value::String(s) if !s.is_empty() => return Ok(()),
                    Value::String(_) => {
                        return Err(VmError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "expected non-empty Id".to_string(),
                        )));
                    }
                    _ => {
                        return Err(VmError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Id, got {type_name}"),
                        )));
                    }
                },
                "Email" => match value {
                    Value::String(s) if rt_validate::is_email(&s) => return Ok(()),
                    Value::String(_) => {
                        return Err(VmError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "invalid email address".to_string(),
                        )));
                    }
                    _ => {
                        return Err(VmError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Email, got {type_name}"),
                        )));
                    }
                },
                "Bytes" => {
                    if matches!(value, Value::Bytes(_)) {
                        return Ok(());
                    }
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bytes, got {type_name}"),
                    )));
                }
                "Html" => {
                    if matches!(value, Value::Html(_)) {
                        return Ok(());
                    }
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Html, got {type_name}"),
                    )));
                }
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
            _ => Err(VmError::Error(self.validation_error_value(
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

    fn list_index(&self, value: &Value) -> VmResult<usize> {
        match value.unboxed() {
            Value::Int(v) if v >= 0 => Ok(v as usize),
            Value::Int(_) => Err(VmError::Runtime("index out of bounds".to_string())),
            _ => Err(VmError::Runtime("list index must be Int".to_string())),
        }
    }

    fn map_key(&self, value: &Value) -> VmResult<String> {
        match value.unboxed() {
            Value::String(key) => Ok(key),
            _ => Err(VmError::Runtime("map keys must be strings".to_string())),
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
    ) -> VmResult<Value> {
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
                    } else if self.program.types.contains_key(simple_name) {
                        self.decode_struct_json(json, simple_name, path)?
                    } else if self.program.enums.contains_key(simple_name) {
                        self.decode_enum_json(json, simple_name, path)?
                    } else {
                        return Err(VmError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("unknown type {}", ident.name),
                        )));
                    }
                } else if self.program.types.contains_key(simple_name) {
                    self.decode_struct_json(json, simple_name, path)?
                } else if self.program.enums.contains_key(simple_name) {
                    self.decode_enum_json(json, simple_name, path)?
                } else {
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("unknown type {}", ident.name),
                    )));
                }
            }
            TypeRefKind::Result { .. } => {
                return Err(VmError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    "Result is not supported for JSON body",
                )));
            }
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(VmError::Runtime(
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
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        "Result is not supported for JSON body",
                    )));
                }
                "List" => {
                    if args.len() != 1 {
                        return Err(VmError::Runtime("List expects 1 type argument".to_string()));
                    }
                    let rt_json::JsonValue::Array(items) = json else {
                        return Err(VmError::Error(self.validation_error_value(
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
                        return Err(VmError::Runtime("Map expects 2 type arguments".to_string()));
                    }
                    let rt_json::JsonValue::Object(items) = json else {
                        return Err(VmError::Error(self.validation_error_value(
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
                    return Err(VmError::Error(self.validation_error_value(
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
    ) -> VmResult<Option<Value>> {
        let value = match name {
            "Int" => match json {
                rt_json::JsonValue::Number(n) if n.fract() == 0.0 => Value::Int(*n as i64),
                _ => {
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Int",
                    )));
                }
            },
            "Float" => match json {
                rt_json::JsonValue::Number(n) => Value::Float(*n),
                _ => {
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Float",
                    )));
                }
            },
            "Bool" => match json {
                rt_json::JsonValue::Bool(v) => Value::Bool(*v),
                _ => {
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Bool",
                    )));
                }
            },
            "String" | "Id" | "Email" => match json {
                rt_json::JsonValue::String(v) => Value::String(v.clone()),
                _ => {
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected String",
                    )));
                }
            },
            "Bytes" => match json {
                rt_json::JsonValue::String(v) => {
                    let bytes = rt_bytes::decode_base64(v).map_err(|msg| {
                        VmError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            format!("invalid Bytes (base64): {msg}"),
                        ))
                    })?;
                    Value::Bytes(bytes)
                }
                _ => {
                    return Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected String",
                    )));
                }
            },
            "Html" => {
                return Err(VmError::Error(self.validation_error_value(
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
    ) -> VmResult<Value> {
        let rt_json::JsonValue::Object(map) = json else {
            return Err(VmError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("expected {name}"),
            )));
        };
        let decl = self.program.types.get(name).ok_or_else(|| {
            VmError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("unknown type {name}"),
            ))
        })?;
        let fields = decl.fields.clone();
        let mut values = HashMap::new();
        for (key, value) in map {
            let field = fields.iter().find(|f| f.name == *key);
            let Some(field_decl) = field else {
                return Err(VmError::Error(self.validation_error_value(
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
            if values.contains_key(&field_decl.name) {
                continue;
            }
            let field_path = format!("{path}.{}", field_decl.name);
            if let Some(default_fn) = &field_decl.default_fn {
                let func = self
                    .program
                    .functions
                    .get(default_fn)
                    .ok_or_else(|| VmError::Runtime(format!("unknown default {default_fn}")))?;
                let value = self.exec_function(func, Vec::new())?;
                self.validate_value(&value, &field_decl.ty, &field_path)?;
                values.insert(field_decl.name.clone(), value);
            } else if self.is_optional_type(&field_decl.ty) {
                values.insert(field_decl.name.clone(), Value::Null);
            } else {
                return Err(VmError::Error(self.validation_error_value(
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
    ) -> VmResult<Value> {
        let rt_json::JsonValue::Object(map) = json else {
            return Err(VmError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("expected {name}"),
            )));
        };
        let Some(rt_json::JsonValue::String(variant_name)) = map.get("type") else {
            return Err(VmError::Error(self.validation_error_value(
                path,
                "missing_field",
                "missing enum type",
            )));
        };
        let decl = self.program.enums.get(name).ok_or_else(|| {
            VmError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("unknown enum {name}"),
            ))
        })?;
        let variants = decl.variants.clone();
        let variant = variants
            .iter()
            .find(|v| v.name == *variant_name)
            .ok_or_else(|| {
                VmError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    format!("unknown variant {variant_name}"),
                ))
            })?;
        let payload = if variant.payload.is_empty() {
            Vec::new()
        } else {
            let data = map.get("data").ok_or_else(|| {
                VmError::Error(self.validation_error_value(
                    path,
                    "missing_field",
                    "missing enum data",
                ))
            })?;
            if variant.payload.len() == 1 {
                vec![self.decode_json_value(data, &variant.payload[0], &format!("{path}.data"))?]
            } else {
                let rt_json::JsonValue::Array(items) = data else {
                    return Err(VmError::Error(self.validation_error_value(
                        &format!("{path}.data"),
                        "type_mismatch",
                        "expected enum payload array",
                    )));
                };
                if items.len() != variant.payload.len() {
                    return Err(VmError::Error(self.validation_error_value(
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

    fn wrap_function_result(&self, func: &Function, value: Value) -> VmResult<Value> {
        if self.is_result_type(func.ret.as_ref()) {
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

    fn is_optional_type(&self, ty: &TypeRef) -> bool {
        match &ty.kind {
            TypeRefKind::Optional(_) => true,
            TypeRefKind::Generic { base, .. } => base.name == "Option",
            _ => false,
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

    fn make_struct(&mut self, name: &str, mut values: HashMap<String, Value>) -> VmResult<Value> {
        let info = self
            .program
            .types
            .get(name)
            .ok_or_else(|| VmError::Runtime(format!("unknown type {name}")))?;
        for field in &info.fields {
            if let Some(value) = values.get(&field.name) {
                let path = format!("{name}.{}", field.name);
                self.validate_value(value, &field.ty, &path)?;
                continue;
            }
            let path = format!("{name}.{}", field.name);
            if let Some(default_fn) = &field.default_fn {
                let func = self
                    .program
                    .functions
                    .get(default_fn)
                    .ok_or_else(|| VmError::Runtime(format!("unknown default {default_fn}")))?;
                let value = self.exec_function(func, Vec::new())?;
                self.validate_value(&value, &field.ty, &path)?;
                values.insert(field.name.clone(), value);
            } else if self.is_optional_type(&field.ty) {
                values.insert(field.name.clone(), Value::Null);
            } else {
                return Err(VmError::Runtime(format!(
                    "missing field {name}.{}",
                    field.name
                )));
            }
        }
        Ok(Value::Struct {
            name: name.to_string(),
            fields: values,
        })
    }

    fn make_enum(&self, name: &str, variant: &str, payload: Vec<Value>) -> VmResult<Value> {
        let info = self
            .program
            .enums
            .get(name)
            .ok_or_else(|| VmError::Runtime(format!("unknown enum {name}")))?;
        let variant_info = info
            .variants
            .iter()
            .find(|v| v.name == variant)
            .ok_or_else(|| VmError::Runtime(format!("unknown variant {name}.{variant}")))?;
        if variant_info.payload.len() != payload.len() {
            return Err(VmError::Runtime(format!(
                "variant {name}.{variant} expects {} value(s), got {}",
                variant_info.payload.len(),
                payload.len()
            )));
        }
        Ok(Value::Enum {
            name: name.to_string(),
            variant: variant.to_string(),
            payload,
        })
    }

    fn match_pattern(
        &self,
        value: &Value,
        pat: &Pattern,
        bindings: &mut HashMap<String, Value>,
    ) -> VmResult<bool> {
        let value = value.unboxed();
        match &pat.kind {
            PatternKind::Wildcard => Ok(true),
            PatternKind::Literal(lit) => Ok(self.literal_matches(&value, lit)),
            PatternKind::Ident(ident) => {
                if let Some(is_match) = self.match_variant_ident(&value, &ident.name) {
                    return Ok(is_match);
                }
                if let Value::Enum { name, .. } = &value {
                    if self.enum_variant_exists(name, &ident.name) {
                        return Ok(false);
                    }
                }
                bindings.insert(ident.name.clone(), value.clone());
                Ok(true)
            }
            PatternKind::EnumVariant { name, args } => {
                self.match_enum_variant(&value, &name.name, args, bindings)
            }
            PatternKind::Struct { name, fields } => {
                self.match_struct_pattern(&value, &name.name, fields, bindings)
            }
        }
    }

    fn match_variant_ident(&self, value: &Value, name: &str) -> Option<bool> {
        let value = value.unboxed();
        match name {
            "None" => Some(matches!(value, Value::Null)),
            "Some" => Some(!matches!(value, Value::Null)),
            "Ok" => Some(matches!(value, Value::ResultOk(_))),
            "Err" => Some(matches!(value, Value::ResultErr(_))),
            _ => match value {
                Value::Enum {
                    name: enum_name,
                    variant,
                    payload,
                } => {
                    if variant == name {
                        let arity = self.enum_variant_arity(&enum_name, &variant).unwrap_or(0);
                        Some(payload.len() == arity && arity == 0)
                    } else {
                        None
                    }
                }
                _ => None,
            },
        }
    }

    fn match_enum_variant(
        &self,
        value: &Value,
        name: &str,
        args: &[Pattern],
        bindings: &mut HashMap<String, Value>,
    ) -> VmResult<bool> {
        let value = value.unboxed();
        match name {
            "Some" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("Some expects 1 pattern".to_string()));
                }
                if matches!(value, Value::Null) {
                    return Ok(false);
                }
                self.match_pattern(&value, &args[0], bindings)
            }
            "None" => {
                if !args.is_empty() {
                    return Err(VmError::Runtime("None expects no patterns".to_string()));
                }
                Ok(matches!(value, Value::Null))
            }
            "Ok" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("Ok expects 1 pattern".to_string()));
                }
                match value {
                    Value::ResultOk(inner) => self.match_pattern(&inner, &args[0], bindings),
                    _ => Ok(false),
                }
            }
            "Err" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("Err expects 1 pattern".to_string()));
                }
                match value {
                    Value::ResultErr(inner) => self.match_pattern(&inner, &args[0], bindings),
                    _ => Ok(false),
                }
            }
            _ => match value {
                Value::Enum {
                    name: enum_name,
                    variant,
                    payload,
                } => {
                    if variant != name {
                        return Ok(false);
                    }
                    let arity = self
                        .enum_variant_arity(&enum_name, &variant)
                        .unwrap_or(payload.len());
                    if args.len() != arity || payload.len() != arity {
                        return Ok(false);
                    }
                    for (arg, val) in args.iter().zip(payload.iter()) {
                        if !self.match_pattern(val, arg, bindings)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                _ => Err(VmError::Runtime(format!(
                    "enum patterns not supported in VM yet: {name}"
                ))),
            },
        }
    }

    fn match_struct_pattern(
        &self,
        value: &Value,
        name: &str,
        fields: &[crate::ast::PatternField],
        bindings: &mut HashMap<String, Value>,
    ) -> VmResult<bool> {
        let value = value.unboxed();
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

    fn enum_variant_arity(&self, enum_name: &str, variant: &str) -> Option<usize> {
        self.program.enums.get(enum_name).and_then(|info| {
            info.variants
                .iter()
                .find(|v| v.name == variant)
                .map(|v| v.payload.len())
        })
    }

    fn enum_variant_exists(&self, enum_name: &str, variant: &str) -> bool {
        self.enum_variant_arity(enum_name, variant).is_some()
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

    fn check_refined(&self, value: &Value, base: &str, args: &[Expr], path: &str) -> VmResult<()> {
        let value = value.unboxed();
        match base {
            "String" => {
                let (min, max) = self.parse_length_range(args)?;
                let len = match value {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(VmError::Runtime(
                            "refined String expects a String".to_string(),
                        ));
                    }
                };
                if rt_validate::check_len(len, min, max) {
                    Ok(())
                } else {
                    Err(VmError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("length {len} out of range {min}..{max}"),
                    )))
                }
            }
            "Int" => {
                let (min, max) = self.parse_int_range(args)?;
                let val = match value {
                    Value::Int(v) => v,
                    _ => return Err(VmError::Runtime("refined Int expects an Int".to_string())),
                };
                if rt_validate::check_int_range(val, min, max) {
                    Ok(())
                } else {
                    Err(VmError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("value {val} out of range {min}..{max}"),
                    )))
                }
            }
            "Float" => {
                let (min, max) = self.parse_float_range(args)?;
                let val = match value {
                    Value::Float(v) => v,
                    _ => {
                        return Err(VmError::Runtime(
                            "refined Float expects a Float".to_string(),
                        ));
                    }
                };
                if rt_validate::check_float_range(val, min, max) {
                    Ok(())
                } else {
                    Err(VmError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("value {val} out of range {min}..{max}"),
                    )))
                }
            }
            _ => Ok(()),
        }
    }

    fn parse_length_range(&self, args: &[Expr]) -> VmResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| VmError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| VmError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_int_range(&self, args: &[Expr]) -> VmResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| VmError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| VmError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_float_range(&self, args: &[Expr]) -> VmResult<(f64, f64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_f64(left)
            .ok_or_else(|| VmError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_f64(right)
            .ok_or_else(|| VmError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn extract_range_args<'b>(&self, args: &'b [Expr]) -> VmResult<(&'b Expr, &'b Expr)> {
        if args.len() == 1 {
            if let ExprKind::Binary {
                op: crate::ast::BinaryOp::Range,
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
        Err(VmError::Runtime(
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

    fn eval_add(&self, left: Value, right: Value) -> VmResult<Value> {
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
            _ => Err(VmError::Runtime("unsupported + operands".to_string())),
        }
    }

    fn eval_arith(&self, op: &Instr, left: Value, right: Value) -> VmResult<Value> {
        let left = left.unboxed();
        let right = right.unboxed();
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => match op {
                Instr::Sub => Ok(Value::Int(a - b)),
                Instr::Mul => Ok(Value::Int(a * b)),
                Instr::Div => Ok(Value::Int(a / b)),
                Instr::Mod => Ok(Value::Int(a % b)),
                _ => Err(VmError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Float(a), Value::Float(b)) => match op {
                Instr::Sub => Ok(Value::Float(a - b)),
                Instr::Mul => Ok(Value::Float(a * b)),
                Instr::Div => Ok(Value::Float(a / b)),
                Instr::Mod => Err(VmError::Runtime("mod not supported for float".to_string())),
                _ => Err(VmError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Int(a), Value::Float(b)) => match op {
                Instr::Sub => Ok(Value::Float(a as f64 - b)),
                Instr::Mul => Ok(Value::Float(a as f64 * b)),
                Instr::Div => Ok(Value::Float(a as f64 / b)),
                Instr::Mod => Err(VmError::Runtime("mod not supported for float".to_string())),
                _ => Err(VmError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Float(a), Value::Int(b)) => match op {
                Instr::Sub => Ok(Value::Float(a - b as f64)),
                Instr::Mul => Ok(Value::Float(a * b as f64)),
                Instr::Div => Ok(Value::Float(a / b as f64)),
                Instr::Mod => Err(VmError::Runtime("mod not supported for float".to_string())),
                _ => Err(VmError::Runtime("unsupported arithmetic op".to_string())),
            },
            _ => Err(VmError::Runtime(
                "unsupported arithmetic operands".to_string(),
            )),
        }
    }

    fn eval_compare(&self, op: &Instr, left: Value, right: Value) -> VmResult<Value> {
        let left = left.unboxed();
        let right = right.unboxed();
        let result = match (left, right) {
            (Value::Int(a), Value::Int(b)) => match op {
                Instr::Eq => a == b,
                Instr::NotEq => a != b,
                Instr::Lt => a < b,
                Instr::LtEq => a <= b,
                Instr::Gt => a > b,
                Instr::GtEq => a >= b,
                _ => return Err(VmError::Runtime("unsupported comparison".to_string())),
            },
            (Value::Float(a), Value::Float(b)) => match op {
                Instr::Eq => a == b,
                Instr::NotEq => a != b,
                Instr::Lt => a < b,
                Instr::LtEq => a <= b,
                Instr::Gt => a > b,
                Instr::GtEq => a >= b,
                _ => return Err(VmError::Runtime("unsupported comparison".to_string())),
            },
            (Value::String(a), Value::String(b)) => match op {
                Instr::Eq => a == b,
                Instr::NotEq => a != b,
                _ => {
                    return Err(VmError::Runtime(
                        "unsupported string comparison".to_string(),
                    ));
                }
            },
            (Value::Bytes(a), Value::Bytes(b)) => match op {
                Instr::Eq => a == b,
                Instr::NotEq => a != b,
                _ => return Err(VmError::Runtime("unsupported bytes comparison".to_string())),
            },
            _ => {
                return Err(VmError::Runtime(
                    "unsupported comparison operands".to_string(),
                ));
            }
        };
        Ok(Value::Bool(result))
    }

    fn eval_bool(&self, op: &Instr, left: Value, right: Value) -> VmResult<Value> {
        let left = self.as_bool(&left)?;
        let right = self.as_bool(&right)?;
        let result = match op {
            Instr::And => left && right,
            Instr::Or => left || right,
            _ => return Err(VmError::Runtime("unsupported boolean op".to_string())),
        };
        Ok(Value::Bool(result))
    }

    fn as_bool(&self, value: &Value) -> VmResult<bool> {
        match value.unboxed() {
            Value::Bool(v) => Ok(v),
            _ => Err(VmError::Runtime("condition must be a Bool".to_string())),
        }
    }
}

struct Frame<'a> {
    code: &'a [Instr],
    ip: usize,
    locals: Vec<Value>,
    stack: Vec<Value>,
}

impl<'a> Frame<'a> {
    fn new(func: &'a Function) -> Self {
        Self {
            code: &func.code,
            ip: 0,
            locals: vec![Value::Unit; func.locals],
            stack: Vec::new(),
        }
    }

    fn pop(&mut self) -> VmResult<Value> {
        self.stack
            .pop()
            .ok_or_else(|| VmError::Runtime("stack underflow".to_string()))
    }

    fn peek(&self) -> VmResult<&Value> {
        self.stack
            .last()
            .ok_or_else(|| VmError::Runtime("stack underflow".to_string()))
    }
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
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
