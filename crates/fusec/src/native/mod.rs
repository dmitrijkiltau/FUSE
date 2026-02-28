mod jit;
pub mod value;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::thread;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use fuse_rt::{config as rt_config, error as rt_error, json as rt_json, validate as rt_validate};

use crate::ast::{Expr, HttpVerb, Ident, TypeRef, TypeRefKind};
use crate::callbind::{ParamBinding, ParamSpec, bind_positional_args};
use crate::interp::{Task, TaskResult, Value, format_error_value};
use crate::ir::{Config, Function, Program as IrProgram, Service, ServiceRoute};
use crate::loader::ModuleRegistry;
use crate::native::value::NativeHeap;
use crate::observability;
use crate::refinement::{
    NumberLiteral, RefinementConstraint, base_is_string_like, parse_constraints,
};
use crate::span::Span;
use jit::{JitCallError, JitRuntime, ObjectArtifactSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeProgram {
    pub version: u32,
    pub ir: IrProgram,
}

impl NativeProgram {
    pub const VERSION: u32 = 1;

    pub fn from_ir(ir: IrProgram) -> Self {
        Self {
            version: Self::VERSION,
            ir,
        }
    }
}

pub const CACHE_VERSION: u32 = 1;

pub fn compile_registry(registry: &ModuleRegistry) -> Result<NativeProgram, Vec<String>> {
    let ir = crate::ir::lower::lower_registry(registry)?;
    Ok(NativeProgram::from_ir(ir))
}

pub struct NativeObject {
    pub object: Vec<u8>,
    pub interned_strings: Vec<String>,
    pub entry_symbol: String,
    pub config_defaults: Vec<ConfigDefaultSymbol>,
}

pub struct ConfigDefaultSymbol {
    pub name: String,
    pub symbol: String,
}

pub fn symbol_for_function(name: &str) -> String {
    jit::jit_symbol(name)
}

fn simple_function_name(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

fn resolve_public_function_name(program: &NativeProgram, name: &str) -> Result<String, String> {
    if program.ir.functions.contains_key(name) || program.ir.apps.contains_key(name) {
        return Ok(name.to_string());
    }
    if name.contains("::") {
        return Err(format!("unknown function {name}"));
    }
    let mut matches = program
        .ir
        .functions
        .keys()
        .filter(|candidate| simple_function_name(candidate) == name);
    let first = matches.next().cloned();
    let second = matches.next();
    match (first, second) {
        (Some(resolved), None) => Ok(resolved),
        (Some(_), Some(_)) => Err(format!(
            "ambiguous function name {name}; use canonical form m<module_id>::{name}"
        )),
        (None, _) => Err(format!("unknown function {name}")),
    }
}

pub fn emit_object_for_app(
    program: &NativeProgram,
    name: Option<&str>,
) -> Result<NativeObject, String> {
    let app = if let Some(name) = name {
        program
            .ir
            .apps
            .get(name)
            .or_else(|| program.ir.apps.values().find(|func| func.name == name))
            .ok_or_else(|| format!("app not found: {name}"))?
    } else {
        program
            .ir
            .apps
            .values()
            .next()
            .ok_or_else(|| "no app found".to_string())?
    };
    let mut config_defaults = Vec::new();
    let mut extra_funcs = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(app.name.clone());
    for config in program.ir.configs.values() {
        for field in &config.fields {
            let Some(fn_name) = &field.default_fn else {
                continue;
            };
            config_defaults.push(ConfigDefaultSymbol {
                name: fn_name.clone(),
                symbol: symbol_for_function(fn_name),
            });
            if seen.insert(fn_name.clone()) {
                let func = program
                    .ir
                    .functions
                    .get(fn_name)
                    .ok_or_else(|| format!("missing config default {fn_name}"))?;
                extra_funcs.push(func);
            }
        }
    }
    let mut funcs = Vec::with_capacity(1 + extra_funcs.len());
    funcs.push(app);
    funcs.extend(extra_funcs);
    let ObjectArtifactSet {
        object,
        interned_strings,
    } = match jit::emit_object_for_functions(&program.ir, &funcs) {
        Ok(artifact) => artifact,
        Err(err) => return Err(err),
    };
    Ok(NativeObject {
        object,
        interned_strings,
        entry_symbol: symbol_for_function(&app.name),
        config_defaults,
    })
}

pub fn emit_object_for_function(
    program: &NativeProgram,
    name: &str,
) -> Result<NativeObject, String> {
    let resolved_name = resolve_public_function_name(program, name)?;
    let func = program
        .ir
        .functions
        .get(&resolved_name)
        .or_else(|| program.ir.apps.get(&resolved_name))
        .or_else(|| {
            program
                .ir
                .apps
                .values()
                .find(|func| func.name == resolved_name)
        })
        .ok_or_else(|| format!("unknown function {name}"))?;
    let artifact = jit::emit_object_for_function(&program.ir, func)?;
    Ok(NativeObject {
        object: artifact.object,
        interned_strings: artifact.interned_strings,
        entry_symbol: artifact.entry_symbol,
        config_defaults: Vec::new(),
    })
}

pub fn load_configs_for_binary<'a, I>(
    configs: I,
    heap: &mut NativeHeap,
    mut default_fn: impl FnMut(&str, &mut NativeHeap) -> Result<Value, String>,
) -> Result<(), String>
where
    I: IntoIterator<Item = &'a Config>,
{
    let mut evaluator = ConfigEvaluator::default();
    evaluator
        .eval_configs(configs, heap, &mut default_fn)
        .map_err(render_native_error)
}

pub fn load_types_for_binary<'a, I>(types: I, heap: &mut NativeHeap) -> Result<(), String>
where
    I: IntoIterator<Item = &'a crate::ir::TypeInfo>,
{
    let mut map = HashMap::new();
    for ty in types {
        map.insert(ty.name.clone(), ty.clone());
    }
    heap.set_types(map);
    Ok(())
}

#[derive(Debug)]
pub(crate) enum NativeError {
    Runtime(String),
    Error(Value),
}

type NativeResult<T> = Result<T, NativeError>;

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

fn render_native_error(err: NativeError) -> String {
    match err {
        NativeError::Runtime(message) => message,
        NativeError::Error(value) => format_error_value(&value),
    }
}

pub struct NativeVm<'a> {
    program: &'a NativeProgram,
    jit: JitRuntime,
    heap: NativeHeap,
    configs_loaded: bool,
    regex_cache: HashMap<String, regex::Regex>,
    current_http_request: Option<HttpRequestContext>,
    current_http_response: Option<HttpResponseMeta>,
}

pub struct NativeRuntimeContextGuard {
    _guard: jit::VmGuard,
}

impl<'a> crate::runtime_types::RuntimeTypeHost for NativeVm<'a> {
    type Error = NativeError;

    fn runtime_error(&self, message: String) -> Self::Error {
        NativeError::Runtime(message)
    }

    fn validation_error(&self, path: &str, code: &str, message: String) -> Self::Error {
        NativeError::Error(crate::runtime_types::validation_error_value(
            path, code, message,
        ))
    }

    fn has_struct_type(&self, name: &str) -> bool {
        self.program.ir.types.contains_key(name)
    }

    fn has_enum_type(&self, name: &str) -> bool {
        self.program.ir.enums.contains_key(name)
    }

    fn decode_struct_type_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> Result<Value, Self::Error> {
        NativeVm::decode_struct_json(self, json, name, path)
    }

    fn decode_enum_type_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> Result<Value, Self::Error> {
        NativeVm::decode_enum_json(self, json, name, path)
    }

    fn check_refined_value(
        &mut self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> Result<(), Self::Error> {
        NativeVm::check_refined(self, value, base, args, path)
    }
}

impl<'a> NativeVm<'a> {
    pub fn new(program: &'a NativeProgram) -> Self {
        let mut heap = NativeHeap::new();
        heap.set_types(program.ir.types.clone());
        Self {
            program,
            jit: JitRuntime::build(),
            heap,
            configs_loaded: false,
            regex_cache: HashMap::new(),
            current_http_request: None,
            current_http_response: None,
        }
    }

    pub fn run_app(&mut self, name: Option<&str>) -> Result<(), String> {
        self.ensure_configs_loaded()?;
        let app = if let Some(name) = name {
            self.program
                .ir
                .apps
                .get(name)
                .ok_or_else(|| format!("app not found: {name}"))?
        } else {
            self.program
                .ir
                .apps
                .values()
                .next()
                .ok_or_else(|| "no app found".to_string())?
        };
        self.call_function_native_only_inner(&app.name, Vec::new())?;
        Ok(())
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        self.ensure_configs_loaded()?;
        let resolved = resolve_public_function_name(self.program, name)?;
        self.call_function_native_only_inner(&resolved, args)
    }

    pub fn has_jit_function(&self, name: &str) -> bool {
        if self.jit.has_function(name) {
            return true;
        }
        resolve_public_function_name(self.program, name)
            .ok()
            .is_some_and(|resolved| self.jit.has_function(&resolved))
    }

    pub fn enter_runtime_context(&mut self) -> NativeRuntimeContextGuard {
        let vm_ptr = self as *mut NativeVm<'_> as *mut NativeVm<'static>;
        NativeRuntimeContextGuard {
            _guard: jit::VmGuard::enter(vm_ptr),
        }
    }

    fn call_function_native_only_inner(
        &mut self,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        let vm_ptr = self as *mut NativeVm<'_> as *mut NativeVm<'static>;
        let _guard = jit::VmGuard::enter(vm_ptr);
        call_function_native_only_with(self.program, &mut self.jit, &mut self.heap, name, args)
    }

    fn ensure_configs_loaded(&mut self) -> Result<(), String> {
        if self.configs_loaded {
            return Ok(());
        }
        self.eval_configs_native().map_err(render_native_error)?;
        self.configs_loaded = true;
        Ok(())
    }

    fn eval_configs_native(&mut self) -> NativeResult<()> {
        let config_path =
            std::env::var("FUSE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        let file_values =
            rt_config::load_config_file(&config_path).map_err(NativeError::Runtime)?;
        let mut configs: Vec<Config> = self.program.ir.configs.values().cloned().collect();
        configs.sort_by(|a, b| a.name.cmp(&b.name));
        for config in configs {
            self.heap.ensure_config(&config.name);
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
                                self.call_function_native_only_inner(fn_name, Vec::new())
                                    .map_err(NativeError::Runtime)?
                            } else {
                                Value::Null
                            }
                        } else if let Some(fn_name) = &field.default_fn {
                            self.call_function_native_only_inner(fn_name, Vec::new())
                                .map_err(NativeError::Runtime)?
                        } else {
                            Value::Null
                        };
                        self.validate_value(&value, &field.ty, &path)?;
                        value
                    }
                };
                self.heap.set_config_field(&config.name, &field.name, value);
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn app_contains_serve(&self, func: &Function) -> bool {
        let mut seen = HashSet::new();
        self.func_contains_serve(func, &mut seen)
    }

    #[allow(dead_code)]
    fn app_needs_interpreter(&self, func: &Function) -> bool {
        self.app_contains_serve(func)
    }

    #[allow(dead_code)]
    fn func_contains_serve(&self, func: &Function, seen: &mut HashSet<String>) -> bool {
        if !seen.insert(func.name.clone()) {
            return false;
        }
        for instr in &func.code {
            match instr {
                crate::ir::Instr::Call {
                    name,
                    kind: crate::ir::CallKind::Builtin,
                    ..
                } if name == "serve" => return true,
                crate::ir::Instr::Call {
                    name,
                    kind: crate::ir::CallKind::Function,
                    ..
                } => {
                    if let Some(callee) = self
                        .program
                        .ir
                        .functions
                        .get(name)
                        .or_else(|| self.program.ir.apps.get(name))
                        .or_else(|| {
                            self.program
                                .ir
                                .apps
                                .values()
                                .find(|func| func.name == name.as_str())
                        })
                    {
                        if self.func_contains_serve(callee, seen) {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    #[allow(dead_code)]
    fn eval_app_interpreter(&mut self, func: &Function) -> Result<(), String> {
        let mut locals = vec![Value::Unit; func.locals];
        let mut stack: Vec<Value> = Vec::new();
        let mut ip = 0usize;
        while ip < func.code.len() {
            match &func.code[ip] {
                crate::ir::Instr::Push(constant) => {
                    let value = match constant {
                        crate::ir::Const::Unit => Value::Unit,
                        crate::ir::Const::Int(v) => Value::Int(*v),
                        crate::ir::Const::Float(v) => Value::Float(*v),
                        crate::ir::Const::Bool(v) => Value::Bool(*v),
                        crate::ir::Const::String(v) => Value::String(v.clone()),
                        crate::ir::Const::Null => Value::Null,
                    };
                    stack.push(value);
                }
                crate::ir::Instr::LoadLocal(slot) => {
                    let value = locals
                        .get(*slot)
                        .cloned()
                        .ok_or_else(|| format!("invalid local {slot}"))?;
                    stack.push(value);
                }
                crate::ir::Instr::StoreLocal(slot) => {
                    let value = stack.pop().ok_or_else(|| "stack underflow".to_string())?;
                    if *slot >= locals.len() {
                        return Err(format!("invalid local {slot}"));
                    }
                    locals[*slot] = value;
                }
                crate::ir::Instr::Pop => {
                    stack.pop().ok_or_else(|| "stack underflow".to_string())?;
                }
                crate::ir::Instr::Dup => {
                    let value = stack
                        .last()
                        .cloned()
                        .ok_or_else(|| "stack underflow".to_string())?;
                    stack.push(value);
                }
                crate::ir::Instr::InterpString { parts } => {
                    let mut items = Vec::with_capacity(*parts);
                    for _ in 0..*parts {
                        items.push(stack.pop().ok_or_else(|| "stack underflow".to_string())?);
                    }
                    items.reverse();
                    let mut out = String::new();
                    for part in items {
                        out.push_str(&part.to_string_value());
                    }
                    stack.push(Value::String(out));
                }
                crate::ir::Instr::LoadConfigField { config, field } => {
                    let value = self
                        .heap
                        .config_field(config, field)
                        .ok_or_else(|| format!("unknown config field {config}.{field}"))?;
                    stack.push(value);
                }
                crate::ir::Instr::Call { name, argc, kind } => {
                    let mut args = Vec::new();
                    for _ in 0..*argc {
                        args.push(stack.pop().ok_or_else(|| "stack underflow".to_string())?);
                    }
                    args.reverse();
                    match kind {
                        crate::ir::CallKind::Builtin => {
                            let args: Vec<Value> =
                                args.into_iter().map(|val| val.unboxed()).collect();
                            match name.as_str() {
                                "serve" => {
                                    let port_value = args.get(0).cloned().unwrap_or(Value::Null);
                                    let port = self.port_from_value(&port_value)?;
                                    let _ = self
                                        .eval_serve_native_inner(port)
                                        .map_err(|err| self.render_native_error(err))?;
                                    return Ok(());
                                }
                                "print" => {
                                    let text = args
                                        .get(0)
                                        .map(|v| v.to_string_value())
                                        .unwrap_or_default();
                                    println!("{text}");
                                    stack.push(Value::Unit);
                                }
                                "input" => {
                                    if args.len() > 1 {
                                        return Err("input expects 0 or 1 arguments".to_string());
                                    }
                                    let prompt = match args.first() {
                                        Some(Value::String(text)) => text.as_str(),
                                        Some(_) => {
                                            return Err("input expects a string prompt".to_string());
                                        }
                                        None => "",
                                    };
                                    let text = crate::runtime_io::read_input_line(prompt)?;
                                    stack.push(Value::String(text));
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
                                                rt_json::JsonValue::String(
                                                    level.json_label().to_string(),
                                                ),
                                            );
                                            obj.insert(
                                                "message".to_string(),
                                                rt_json::JsonValue::String(message),
                                            );
                                            obj.insert("data".to_string(), data_json);
                                            eprintln!(
                                                "{}",
                                                rt_json::encode(&rt_json::JsonValue::Object(obj))
                                            );
                                        } else {
                                            let message = args[start_idx..]
                                                .iter()
                                                .map(|val| val.to_string_value())
                                                .collect::<Vec<_>>()
                                                .join(" ");
                                            eprintln!(
                                                "{}",
                                                crate::runtime_io::format_log_text_line(
                                                    level.label(),
                                                    &message,
                                                )
                                            );
                                        }
                                    }
                                    stack.push(Value::Unit);
                                }
                                other => {
                                    return Err(format!(
                                        "native backend unsupported: builtin {other}"
                                    ));
                                }
                            }
                        }
                        crate::ir::CallKind::Function => {
                            let value = self.call_function_native_only_inner(name, args)?;
                            stack.push(value);
                        }
                    }
                }
                crate::ir::Instr::Spawn { name, argc } => {
                    let mut args = Vec::new();
                    for _ in 0..*argc {
                        args.push(stack.pop().ok_or_else(|| "stack underflow".to_string())?);
                    }
                    args.reverse();
                    let task_name = name.clone();
                    let program = NativeProgram::from_ir(self.program.ir.clone());
                    let configs = self.heap.clone_configs();
                    let task = Task::spawn_async(move || {
                        run_native_spawn_task(program, configs, task_name, args)
                    });
                    stack.push(Value::Task(task));
                }
                crate::ir::Instr::Await => {
                    let value = stack.pop().ok_or_else(|| "stack underflow".to_string())?;
                    match value {
                        Value::Task(task) => match task.result_raw() {
                            TaskResult::Ok(value) => stack.push(value),
                            TaskResult::Error(err) => return Err(format_error_value(&err)),
                            TaskResult::Runtime(msg) => return Err(msg),
                        },
                        _ => {
                            return Err("await expects a Task value".to_string());
                        }
                    }
                }
                crate::ir::Instr::Return => {
                    return Ok(());
                }
                other => {
                    return Err(format!(
                        "native backend unsupported: app instruction {other:?}"
                    ));
                }
            }
            ip += 1;
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn port_from_value(&self, value: &Value) -> Result<i64, String> {
        match value.unboxed() {
            Value::Int(v) => Ok(v),
            Value::Float(v) => Ok(v as i64),
            Value::String(s) => s
                .parse::<i64>()
                .map_err(|_| "serve expects a port number".to_string()),
            _ => Err("serve expects a port number".to_string()),
        }
    }

    fn eval_serve_native_inner(&mut self, port: i64) -> NativeResult<Value> {
        self.ensure_configs_loaded().map_err(NativeError::Runtime)?;
        let service = self.select_service()?.clone();
        let host = std::env::var("FUSE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = port
            .try_into()
            .map_err(|_| NativeError::Runtime("invalid port".to_string()))?;
        let addr = format!("{host}:{port}");
        let listener = TcpListener::bind(&addr)
            .map_err(|err| NativeError::Runtime(format!("failed to bind {addr}: {err}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|err| NativeError::Runtime(format!("failed to configure {addr}: {err}")))?;
        observability::begin_graceful_shutdown_session();
        let max_requests = std::env::var("FUSE_MAX_REQUESTS")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(0);
        let mut handled = 0usize;
        loop {
            if observability::graceful_shutdown_requested() {
                let signal = observability::take_shutdown_signal_name().unwrap_or("unknown");
                eprintln!(
                    "shutdown: runtime=native signal={signal} handled_requests={handled}"
                );
                break;
            }
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(err) => match err.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => {
                        thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                    _ => {
                        return Err(NativeError::Runtime(format!(
                            "failed to accept connection: {err}"
                        )));
                    }
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
                "native",
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
        Ok(Value::Unit)
    }

    fn render_native_error(&self, err: NativeError) -> String {
        render_native_error(err)
    }

    fn select_service(&self) -> NativeResult<&Service> {
        if self.program.ir.services.is_empty() {
            return Err(NativeError::Runtime("no service declared".to_string()));
        }
        if let Ok(name) = std::env::var("FUSE_SERVICE") {
            return self
                .program
                .ir
                .services
                .get(&name)
                .ok_or_else(|| NativeError::Runtime(format!("service not found: {name}")));
        }
        if self.program.ir.services.len() == 1 {
            return Ok(self.program.ir.services.values().next().unwrap());
        }
        Err(NativeError::Runtime(
            "multiple services declared; set FUSE_SERVICE".to_string(),
        ))
    }

    fn handle_http_request(
        &mut self,
        service: &Service,
        request: &HttpRequest,
    ) -> NativeResult<String> {
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
                return Err(NativeError::Error(self.validation_error_value(
                    "body",
                    "missing_field",
                    "missing JSON body",
                )));
            }
            let json = rt_json::decode(&body_text).map_err(|msg| {
                NativeError::Error(self.validation_error_value("body", "invalid_json", msg))
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

    pub(crate) fn request_header(&self, name: &str) -> Result<Option<String>, String> {
        let request = self.current_http_request.as_ref().ok_or_else(|| {
            "request.header is only available while handling an HTTP route".to_string()
        })?;
        Ok(request.headers.get(&name.to_ascii_lowercase()).cloned())
    }

    pub(crate) fn request_cookie(&self, name: &str) -> Result<Option<String>, String> {
        let request = self.current_http_request.as_ref().ok_or_else(|| {
            "request.cookie is only available while handling an HTTP route".to_string()
        })?;
        Ok(request.cookies.get(name).cloned())
    }

    pub(crate) fn response_add_header(&mut self, name: &str, value: &str) -> Result<(), String> {
        validate_http_header(name, value)?;
        let response = self.current_http_response.as_mut().ok_or_else(|| {
            "response.header is only available while handling an HTTP route".to_string()
        })?;
        response.headers.push((name.to_string(), value.to_string()));
        Ok(())
    }

    pub(crate) fn response_set_cookie(&mut self, name: &str, value: &str) -> Result<(), String> {
        validate_cookie_name(name)?;
        validate_cookie_value(value)?;
        let response = self.current_http_response.as_mut().ok_or_else(|| {
            "response.cookie is only available while handling an HTTP route".to_string()
        })?;
        response
            .cookies
            .push(format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax"));
        Ok(())
    }

    pub(crate) fn response_delete_cookie(&mut self, name: &str) -> Result<(), String> {
        validate_cookie_name(name)?;
        let response = self.current_http_response.as_mut().ok_or_else(|| {
            "response.delete_cookie is only available while handling an HTTP route".to_string()
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
        route: &ServiceRoute,
        mut params: Vec<Value>,
        body_value: Option<Value>,
    ) -> NativeResult<Value> {
        if let Some(body) = body_value {
            params.push(body);
        }
        self.call_function_native_only_inner(&route.handler, params)
            .map_err(NativeError::Runtime)
    }

    fn match_route<'r>(
        &mut self,
        service: &'r Service,
        verb: &HttpVerb,
        path: &str,
    ) -> NativeResult<Option<(&'r ServiceRoute, Vec<Value>)>> {
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

    fn read_http_request(&self, stream: &mut TcpStream) -> NativeResult<HttpRequest> {
        let mut buffer = Vec::new();
        let mut temp = [0u8; 1024];
        let mut header_end = None;
        loop {
            let read = stream
                .read(&mut temp)
                .map_err(|err| NativeError::Runtime(format!("failed to read request: {err}")))?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..read]);
            if let Some(pos) = find_header_end(&buffer) {
                header_end = Some(pos);
                break;
            }
            if buffer.len() > 1024 * 1024 {
                return Err(NativeError::Runtime("request header too large".to_string()));
            }
        }
        let header_end = header_end.ok_or_else(|| {
            NativeError::Runtime("invalid HTTP request: missing headers".to_string())
        })?;
        let header_bytes = &buffer[..header_end];
        let header_text = String::from_utf8_lossy(header_bytes);
        let mut lines = header_text.split("\r\n");
        let request_line = lines
            .next()
            .ok_or_else(|| NativeError::Runtime("invalid HTTP request line".to_string()))?;
        let mut parts = request_line.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| NativeError::Runtime("invalid HTTP request line".to_string()))?
            .to_string();
        let path = parts
            .next()
            .ok_or_else(|| NativeError::Runtime("invalid HTTP request line".to_string()))?
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
                .map_err(|err| NativeError::Runtime(format!("failed to read body: {err}")))?;
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

    fn http_error_response(&self, err: NativeError) -> String {
        match err {
            NativeError::Error(value) => {
                let status = self.http_status_for_error_value(&value);
                let body = self.error_json_from_value(&value);
                self.http_response(status, body)
            }
            NativeError::Runtime(message) => {
                let body = self.internal_error_json(&message);
                self.http_response(500, body)
            }
        }
    }

    fn http_error_response_for_request(&self, request: &HttpRequest, err: NativeError) -> String {
        match err {
            NativeError::Error(value) => {
                let status = self.http_status_for_error_value(&value);
                let body = self.error_json_from_value(&value);
                self.http_response_for_request(request, status, body)
            }
            NativeError::Runtime(message) => {
                let body = self.internal_error_json(&message);
                self.http_response_for_request(request, 500, body)
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

    fn render_html_value(&self, value: &Value) -> NativeResult<String> {
        match value.unboxed() {
            Value::Html(node) => Ok(node.render_to_string()),
            other => Err(NativeError::Runtime(format!(
                "expected Html response, got {}",
                self.value_type_name(&other)
            ))),
        }
    }

    fn parse_env_value(&mut self, ty: &TypeRef, raw: &str) -> NativeResult<Value> {
        crate::runtime_types::parse_env_value(self, ty, raw)
    }

    fn map_parse_error(&self, err: NativeError, path: &str) -> NativeError {
        match err {
            NativeError::Runtime(message) => {
                NativeError::Error(self.validation_error_value(path, "invalid_value", message))
            }
            other => other,
        }
    }

    fn validate_value(&mut self, value: &Value, ty: &TypeRef, path: &str) -> NativeResult<()> {
        crate::runtime_types::validate_value(self, value, ty, path)
    }

    fn value_type_name(&self, value: &Value) -> String {
        crate::runtime_types::value_type_name(value)
    }

    fn is_optional_type(&self, ty: &TypeRef) -> bool {
        crate::runtime_types::is_optional_type(ty)
    }

    fn validation_error_value(&self, path: &str, code: &str, message: impl Into<String>) -> Value {
        crate::runtime_types::validation_error_value(path, code, message)
    }

    fn check_refined(
        &mut self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> NativeResult<()> {
        let constraints = parse_constraints(args).map_err(|err| {
            NativeError::Runtime(format!("invalid refined constraint: {}", err.message))
        })?;
        for constraint in constraints {
            match constraint {
                RefinementConstraint::Range { min, max, .. } => {
                    self.check_refined_range(value, base, min, max, path)?;
                }
                RefinementConstraint::Regex { pattern, .. } => {
                    if !base_is_string_like(base) {
                        return Err(NativeError::Runtime(format!(
                            "regex() constraint is not supported for refined {base}"
                        )));
                    }
                    let Value::String(text) = value.unboxed() else {
                        return Err(NativeError::Runtime(
                            "refined String expects a String".to_string(),
                        ));
                    };
                    if !self.regex_matches(&pattern, &text)? {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            format!("value does not match regex {pattern}"),
                        )));
                    }
                }
                RefinementConstraint::Predicate { name, .. } => {
                    if !self.eval_refinement_predicate(&name, value)? {
                        return Err(NativeError::Error(self.validation_error_value(
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
    ) -> NativeResult<()> {
        match base {
            "String" | "Id" | "Email" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let len = match value.unboxed() {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined String expects a String".to_string(),
                        ));
                    }
                };
                if rt_validate::check_len(len, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("length {len} out of range {min}..{max}"),
                    )))
                }
            }
            "Bytes" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let len = match value.unboxed() {
                    Value::Bytes(bytes) => bytes.len() as i64,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Bytes expects Bytes".to_string(),
                        ));
                    }
                };
                if rt_validate::check_len(len, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("length {len} out of range {min}..{max}"),
                    )))
                }
            }
            "Int" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let val = match value.unboxed() {
                    Value::Int(v) => v,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Int expects an Int".to_string(),
                        ));
                    }
                };
                if rt_validate::check_int_range(val, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
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
                        return Err(NativeError::Runtime(
                            "refined Float expects a Float".to_string(),
                        ));
                    }
                };
                if rt_validate::check_float_range(val, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("value {val} out of range {min}..{max}"),
                    )))
                }
            }
            _ => Err(NativeError::Runtime(format!(
                "range constraint is not supported for refined {base}"
            ))),
        }
    }

    fn regex_matches(&mut self, pattern: &str, text: &str) -> NativeResult<bool> {
        if !self.regex_cache.contains_key(pattern) {
            let compiled = regex::Regex::new(pattern).map_err(|err| {
                NativeError::Runtime(format!("invalid regex pattern {pattern}: {err}"))
            })?;
            self.regex_cache.insert(pattern.to_string(), compiled);
        }
        let regex = self
            .regex_cache
            .get(pattern)
            .ok_or_else(|| NativeError::Runtime("regex cache error".to_string()))?;
        Ok(regex.is_match(text))
    }

    fn eval_refinement_predicate(&mut self, fn_name: &str, value: &Value) -> NativeResult<bool> {
        let resolved = resolve_public_function_name(self.program, fn_name)
            .unwrap_or_else(|_| fn_name.to_string());
        let result = self
            .call_function_native_only_inner(&resolved, vec![value.clone()])
            .map_err(NativeError::Runtime)?;
        match result.unboxed() {
            Value::Bool(ok) => Ok(ok),
            _ => Err(NativeError::Runtime(format!(
                "predicate {fn_name} must return Bool"
            ))),
        }
    }

    fn value_to_json(&self, value: &Value) -> rt_json::JsonValue {
        crate::runtime_types::value_to_json(value)
    }

    fn decode_json_value(
        &mut self,
        json: &rt_json::JsonValue,
        ty: &TypeRef,
        path: &str,
    ) -> NativeResult<Value> {
        crate::runtime_types::decode_json_value(self, json, ty, path)
    }

    fn decode_struct_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> NativeResult<Value> {
        let rt_json::JsonValue::Object(map) = json else {
            return Err(NativeError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("expected {name}"),
            )));
        };
        let decl = self.program.ir.types.get(name).ok_or_else(|| {
            NativeError::Error(self.validation_error_value(
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
                return Err(NativeError::Error(self.validation_error_value(
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
                let value = self
                    .call_function_native_only_inner(default_fn, Vec::new())
                    .map_err(NativeError::Runtime)?;
                self.validate_value(&value, &field_decl.ty, &field_path)?;
                values.insert(field_decl.name.clone(), value);
            } else if self.is_optional_type(&field_decl.ty) {
                values.insert(field_decl.name.clone(), Value::Null);
            } else {
                return Err(NativeError::Error(self.validation_error_value(
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
    ) -> NativeResult<Value> {
        let rt_json::JsonValue::Object(map) = json else {
            return Err(NativeError::Error(self.validation_error_value(
                path,
                "type_mismatch",
                format!("expected {name}"),
            )));
        };
        let Some(rt_json::JsonValue::String(variant_name)) = map.get("type") else {
            return Err(NativeError::Error(self.validation_error_value(
                path,
                "missing_field",
                "missing enum type",
            )));
        };
        let decl = self.program.ir.enums.get(name).ok_or_else(|| {
            NativeError::Error(self.validation_error_value(
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
                NativeError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    format!("unknown variant {variant_name}"),
                ))
            })?;
        let payload = if variant.payload.is_empty() {
            Vec::new()
        } else {
            let data = map.get("data").ok_or_else(|| {
                NativeError::Error(self.validation_error_value(
                    path,
                    "missing_field",
                    "missing enum data",
                ))
            })?;
            if variant.payload.len() == 1 {
                vec![self.decode_json_value(data, &variant.payload[0], &format!("{path}.data"))?]
            } else {
                let rt_json::JsonValue::Array(items) = data else {
                    return Err(NativeError::Error(self.validation_error_value(
                        &format!("{path}.data"),
                        "type_mismatch",
                        "expected enum payload array",
                    )));
                };
                if items.len() != variant.payload.len() {
                    return Err(NativeError::Error(self.validation_error_value(
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
}

fn call_function_native_only_with(
    program: &NativeProgram,
    jit: &mut JitRuntime,
    heap: &mut NativeHeap,
    name: &str,
    args: Vec<Value>,
) -> Result<Value, String> {
    if !heap.has_types() {
        heap.set_types(program.ir.types.clone());
    }
    let func = program
        .ir
        .functions
        .get(name)
        .or_else(|| program.ir.apps.get(name))
        .or_else(|| program.ir.apps.values().find(|func| func.name == name))
        .ok_or_else(|| format!("unknown function {name}"))?;
    let param_specs: Vec<ParamSpec<'_>> = func
        .params
        .iter()
        .map(|param| ParamSpec {
            name: param.as_str(),
            has_default: false,
        })
        .collect();
    let (plan, bind_errors) = bind_positional_args(&param_specs, args.len());
    if !bind_errors.is_empty()
        || plan
            .param_bindings
            .iter()
            .any(|binding| matches!(binding, ParamBinding::MissingRequired))
    {
        return Err(format!(
            "invalid call to {name}: expected {} args, got {}",
            func.params.len(),
            args.len()
        ));
    }
    if let Some(result) = jit.try_call(&program.ir, name, &args, heap) {
        let out = match result {
            Ok(value) => Ok(wrap_function_result(func, value)),
            Err(JitCallError::Error(err_val)) => {
                if let Err(rollback_err) = heap.rollback_db_transaction() {
                    heap.collect_garbage();
                    return Err(format!("db rollback failed: {rollback_err}"));
                }
                if is_result_type(func.ret.as_ref()) {
                    Ok(Value::ResultErr(Box::new(err_val)))
                } else {
                    Err(format_error_value(&err_val))
                }
            }
            Err(JitCallError::Runtime(message)) => {
                if let Err(rollback_err) = heap.rollback_db_transaction() {
                    heap.collect_garbage();
                    return Err(format!("db rollback failed: {rollback_err}"));
                }
                Err(message)
            }
            Err(JitCallError::Compile(message)) => {
                if let Err(rollback_err) = heap.rollback_db_transaction() {
                    heap.collect_garbage();
                    return Err(format!("db rollback failed: {rollback_err}"));
                }
                Err(message)
            }
        };
        heap.collect_garbage();
        return out;
    }
    heap.collect_garbage();
    Err(format!(
        "native backend could not compile function {}",
        func.name
    ))
}

fn run_native_spawn_task(
    program: NativeProgram,
    configs: HashMap<String, HashMap<String, Value>>,
    name: String,
    args: Vec<Value>,
) -> TaskResult {
    let mut vm = NativeVm::new(&program);
    vm.heap.set_configs(configs);
    vm.configs_loaded = true;
    match vm.call_function(&name, args) {
        Ok(value) => TaskResult::Ok(value),
        Err(msg) => TaskResult::Runtime(msg),
    }
}

fn wrap_function_result(func: &Function, value: Value) -> Value {
    if is_result_type(func.ret.as_ref()) {
        match value {
            Value::ResultOk(_) | Value::ResultErr(_) => value,
            _ => Value::ResultOk(Box::new(value)),
        }
    } else {
        value
    }
}

fn is_result_type(ty: Option<&crate::ast::TypeRef>) -> bool {
    match ty {
        Some(ty) => match &ty.kind {
            crate::ast::TypeRefKind::Result { .. } => true,
            crate::ast::TypeRefKind::Generic { base, .. } => base.name == "Result",
            _ => false,
        },
        None => false,
    }
}

#[derive(Default)]
struct ConfigEvaluator {
    regex_cache: HashMap<String, regex::Regex>,
}

impl crate::runtime_types::RuntimeTypeHost for ConfigEvaluator {
    type Error = NativeError;

    fn runtime_error(&self, message: String) -> Self::Error {
        NativeError::Runtime(message)
    }

    fn validation_error(&self, path: &str, code: &str, message: String) -> Self::Error {
        NativeError::Error(crate::runtime_types::validation_error_value(
            path, code, message,
        ))
    }

    fn has_struct_type(&self, _name: &str) -> bool {
        false
    }

    fn has_enum_type(&self, _name: &str) -> bool {
        false
    }

    fn decode_struct_type_json(
        &mut self,
        _json: &rt_json::JsonValue,
        _name: &str,
        path: &str,
    ) -> Result<Value, Self::Error> {
        Err(self.validation_error(
            path,
            "invalid_value",
            "user-defined types are not supported for config env overrides".to_string(),
        ))
    }

    fn decode_enum_type_json(
        &mut self,
        _json: &rt_json::JsonValue,
        _name: &str,
        path: &str,
    ) -> Result<Value, Self::Error> {
        Err(self.validation_error(
            path,
            "invalid_value",
            "user-defined types are not supported for config env overrides".to_string(),
        ))
    }

    fn check_refined_value(
        &mut self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> Result<(), Self::Error> {
        ConfigEvaluator::check_refined(self, value, base, args, path)
    }
}

impl ConfigEvaluator {
    fn eval_configs<'a, I>(
        &mut self,
        configs: I,
        heap: &mut NativeHeap,
        default_fn: &mut dyn FnMut(&str, &mut NativeHeap) -> Result<Value, String>,
    ) -> NativeResult<()>
    where
        I: IntoIterator<Item = &'a Config>,
    {
        let config_path =
            std::env::var("FUSE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        let file_values =
            rt_config::load_config_file(&config_path).map_err(NativeError::Runtime)?;
        for config in configs {
            heap.ensure_config(&config.name);
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
                                default_fn(fn_name, heap).map_err(NativeError::Runtime)?
                            } else {
                                Value::Null
                            }
                        } else if let Some(fn_name) = &field.default_fn {
                            default_fn(fn_name, heap).map_err(NativeError::Runtime)?
                        } else {
                            Value::Null
                        };
                        self.validate_value(&value, &field.ty, &path)?;
                        value
                    }
                };
                heap.set_config_field(&config.name, &field.name, value);
            }
        }
        Ok(())
    }

    fn parse_env_value(&mut self, ty: &TypeRef, raw: &str) -> NativeResult<Value> {
        crate::runtime_types::parse_env_value(self, ty, raw)
    }

    fn map_parse_error(&self, err: NativeError, path: &str) -> NativeError {
        match err {
            NativeError::Runtime(message) => {
                NativeError::Error(self.validation_error_value(path, "invalid_value", message))
            }
            other => other,
        }
    }

    fn validate_value(&mut self, value: &Value, ty: &TypeRef, path: &str) -> NativeResult<()> {
        crate::runtime_types::validate_value(self, value, ty, path)
    }

    fn validation_error_value(&self, path: &str, code: &str, message: impl Into<String>) -> Value {
        crate::runtime_types::validation_error_value(path, code, message)
    }

    fn check_refined(
        &mut self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> NativeResult<()> {
        let constraints = parse_constraints(args).map_err(|err| {
            NativeError::Runtime(format!("invalid refined constraint: {}", err.message))
        })?;
        for constraint in constraints {
            match constraint {
                RefinementConstraint::Range { min, max, .. } => {
                    self.check_refined_range(value, base, min, max, path)?;
                }
                RefinementConstraint::Regex { pattern, .. } => {
                    if !base_is_string_like(base) {
                        return Err(NativeError::Runtime(format!(
                            "regex() constraint is not supported for refined {base}"
                        )));
                    }
                    let Value::String(text) = value.unboxed() else {
                        return Err(NativeError::Runtime(
                            "refined String expects a String".to_string(),
                        ));
                    };
                    if !self.regex_matches(&pattern, &text)? {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            format!("value does not match regex {pattern}"),
                        )));
                    }
                }
                RefinementConstraint::Predicate { name, .. } => {
                    if !self.eval_refinement_predicate(&name, value)? {
                        return Err(NativeError::Error(self.validation_error_value(
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
    ) -> NativeResult<()> {
        match base {
            "String" | "Id" | "Email" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let len = match value.unboxed() {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined String expects a String".to_string(),
                        ));
                    }
                };
                if rt_validate::check_len(len, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("length {len} out of range {min}..{max}"),
                    )))
                }
            }
            "Bytes" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let len = match value.unboxed() {
                    Value::Bytes(bytes) => bytes.len() as i64,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Bytes expects Bytes".to_string(),
                        ));
                    }
                };
                if rt_validate::check_len(len, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("length {len} out of range {min}..{max}"),
                    )))
                }
            }
            "Int" => {
                let min = min
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let max = max
                    .as_i64()
                    .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
                let val = match value.unboxed() {
                    Value::Int(v) => v,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Int expects an Int".to_string(),
                        ));
                    }
                };
                if rt_validate::check_int_range(val, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
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
                        return Err(NativeError::Runtime(
                            "refined Float expects a Float".to_string(),
                        ));
                    }
                };
                if rt_validate::check_float_range(val, min, max) {
                    Ok(())
                } else {
                    Err(NativeError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        format!("value {val} out of range {min}..{max}"),
                    )))
                }
            }
            _ => Err(NativeError::Runtime(format!(
                "range constraint is not supported for refined {base}"
            ))),
        }
    }

    fn regex_matches(&mut self, pattern: &str, text: &str) -> NativeResult<bool> {
        if !self.regex_cache.contains_key(pattern) {
            let compiled = regex::Regex::new(pattern).map_err(|err| {
                NativeError::Runtime(format!("invalid regex pattern {pattern}: {err}"))
            })?;
            self.regex_cache.insert(pattern.to_string(), compiled);
        }
        let regex = self
            .regex_cache
            .get(pattern)
            .ok_or_else(|| NativeError::Runtime("regex cache error".to_string()))?;
        Ok(regex.is_match(text))
    }

    fn eval_refinement_predicate(&mut self, fn_name: &str, value: &Value) -> NativeResult<bool> {
        let vm = jit::current_vm().ok_or_else(|| {
            NativeError::Runtime("predicate evaluation requires an active native VM".to_string())
        })?;
        let resolved = resolve_public_function_name(vm.program, fn_name)
            .unwrap_or_else(|_| fn_name.to_string());
        let result = vm
            .call_function_native_only_inner(&resolved, vec![value.clone()])
            .map_err(NativeError::Runtime)?;
        match result.unboxed() {
            Value::Bool(ok) => Ok(ok),
            _ => Err(NativeError::Runtime(format!(
                "predicate {fn_name} must return Bool"
            ))),
        }
    }
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

fn validate_http_header(name: &str, value: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("response.header requires a non-empty name".to_string());
    }
    if name.contains('\r') || name.contains('\n') || value.contains('\r') || value.contains('\n') {
        return Err("response.header rejects CR/LF characters".to_string());
    }
    Ok(())
}

fn validate_cookie_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("cookie name must not be empty".to_string());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err("cookie name contains unsupported characters".to_string());
    }
    Ok(())
}

fn validate_cookie_value(value: &str) -> Result<(), String> {
    if value.contains(';') || value.contains('\r') || value.contains('\n') {
        return Err("cookie value contains unsupported characters".to_string());
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
