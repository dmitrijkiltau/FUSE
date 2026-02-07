mod jit;
pub mod value;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use serde::{Deserialize, Serialize};

use fuse_rt::{config as rt_config, error as rt_error, json as rt_json, validate as rt_validate};

use crate::ast::{BinaryOp, Expr, ExprKind, HttpVerb, Ident, Literal, TypeRef, TypeRefKind, UnaryOp};
use crate::interp::{format_error_value, Value};
use crate::ir::{Config, Function, Program as IrProgram, Service, ServiceRoute};
use crate::loader::ModuleRegistry;
use crate::native::value::NativeHeap;
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
        Err(err) => {
            if let Some(reason) = unsupported_reason(&program.ir, app) {
                return Err(format!("native backend unsupported: {reason}"));
            }
            return Err(err);
        }
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
    let func = program
        .ir
        .functions
        .get(name)
        .or_else(|| program.ir.apps.get(name))
        .or_else(|| program.ir.apps.values().find(|func| func.name == name))
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
    let evaluator = ConfigEvaluator;
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
enum NativeError {
    Runtime(String),
    Error(Value),
}

type NativeResult<T> = Result<T, NativeError>;

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
        if self.app_contains_serve(app) {
            return self.eval_app_interpreter(app);
        }
        if self.app_needs_interpreter(app) {
            return self.eval_app_interpreter(app);
        }
        self.call_function_native_only_inner(&app.name, Vec::new())?;
        Ok(())
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        self.ensure_configs_loaded()?;
        self.call_function_native_only_inner(name, args)
    }

    pub fn has_jit_function(&self, name: &str) -> bool {
        self.jit.has_function(name)
    }

    fn call_function_native_only_inner(
        &mut self,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        call_function_native_only_with(
            self.program,
            &mut self.jit,
            &mut self.heap,
            name,
            args,
        )
    }

    fn ensure_configs_loaded(&mut self) -> Result<(), String> {
        if self.configs_loaded {
            return Ok(());
        }
        self.eval_configs_native()
            .map_err(render_native_error)?;
        self.configs_loaded = true;
        Ok(())
    }

    fn eval_configs_native(&mut self) -> NativeResult<()> {
        let evaluator = ConfigEvaluator;
        let mut configs: Vec<Config> = self.program.ir.configs.values().cloned().collect();
        configs.sort_by(|a, b| a.name.cmp(&b.name));
        let program = self.program;
        let jit = &mut self.jit;
        let heap = &mut self.heap;
        evaluator.eval_configs(configs.iter(), heap, &mut |fn_name, heap| {
            call_function_native_only_with(program, jit, heap, fn_name, Vec::new())
        })
    }

    fn app_contains_serve(&self, func: &Function) -> bool {
        func.code.iter().any(|instr| {
            matches!(
                instr,
                crate::ir::Instr::Call {
                    name,
                    kind: crate::ir::CallKind::Builtin,
                    ..
                } if name == "serve"
            )
        })
    }

    fn app_needs_interpreter(&self, func: &Function) -> bool {
        func.code.iter().any(|instr| match instr {
            crate::ir::Instr::Call {
                name,
                kind: crate::ir::CallKind::Builtin,
                ..
            } if name == "serve" => true,
            _ => false,
        })
    }

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
                    let value = stack.last().cloned().ok_or_else(|| "stack underflow".to_string())?;
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
                        crate::ir::CallKind::Builtin => match name.as_str() {
                            "serve" => {
                                let port_value = args.get(0).cloned().unwrap_or(Value::Null);
                                let port = self.port_from_value(&port_value)?;
                                let _ = self.eval_serve_native_inner(port)
                                    .map_err(|err| self.render_native_error(err))?;
                                return Ok(());
                            }
                            "print" => {
                                let text = args.get(0).map(|v| v.to_string_value()).unwrap_or_default();
                                println!("{text}");
                                stack.push(Value::Unit);
                            }
                            other => {
                                return Err(format!(
                                    "native backend unsupported: builtin {other}"
                                ))
                            }
                        },
                        crate::ir::CallKind::Function => {
                            let value = self.call_function_native_only_inner(name, args)?;
                            stack.push(value);
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

    fn port_from_value(&self, value: &Value) -> Result<i64, String> {
        match value.unboxed() {
            Value::Int(v) => Ok(v),
            Value::Float(v) => Ok(v as i64),
            Value::String(s) => s.parse::<i64>().map_err(|_| "serve expects a port number".to_string()),
            _ => Err("serve expects a port number".to_string()),
        }
    }

    fn eval_serve_native_inner(&mut self, port: i64) -> NativeResult<Value> {
        self.ensure_configs_loaded()
            .map_err(NativeError::Runtime)?;
        let service = self.select_service()?.clone();
        let host = std::env::var("FUSE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = port
            .try_into()
            .map_err(|_| NativeError::Runtime("invalid port".to_string()))?;
        let addr = format!("{host}:{port}");
        let listener = TcpListener::bind(&addr)
            .map_err(|err| NativeError::Runtime(format!("failed to bind {addr}: {err}")))?;
        let max_requests = std::env::var("FUSE_MAX_REQUESTS")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(0);
        let mut handled = 0usize;
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    return Err(NativeError::Runtime(format!(
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
        stream: &mut TcpStream,
    ) -> NativeResult<String> {
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

    fn parse_env_value(&self, ty: &TypeRef, raw: &str) -> NativeResult<Value> {
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
            TypeRefKind::Result { .. } => Err(NativeError::Runtime(
                "Result is not supported for config env overrides".to_string(),
            )),
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(NativeError::Runtime(
                            "Option expects 1 type argument".to_string(),
                        ));
                    }
                    if raw.eq_ignore_ascii_case("null") || raw.is_empty() {
                        Ok(Value::Null)
                    } else {
                        self.parse_env_value(&args[0], raw)
                    }
                }
                "Result" => Err(NativeError::Runtime(
                    "Result is not supported for config env overrides".to_string(),
                )),
                _ => Err(NativeError::Runtime(
                    "config env overrides only support simple types".to_string(),
                )),
            },
        }
    }

    fn parse_simple_env(&self, name: &str, raw: &str) -> NativeResult<Value> {
        match name {
            "Int" => raw
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| NativeError::Runtime(format!("invalid Int: {raw}"))),
            "Float" => raw
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| NativeError::Runtime(format!("invalid Float: {raw}"))),
            "Bool" => match raw.to_ascii_lowercase().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(NativeError::Runtime(format!("invalid Bool: {raw}"))),
            },
            "String" | "Id" | "Email" | "Bytes" => Ok(Value::String(raw.to_string())),
            _ => Err(NativeError::Runtime(format!(
                "env override not supported for type {name}"
            ))),
        }
    }

    fn map_parse_error(&self, err: NativeError, path: &str) -> NativeError {
        match err {
            NativeError::Runtime(message) => {
                NativeError::Error(self.validation_error_value(path, "invalid_value", message))
            }
            other => other,
        }
    }

    fn validate_value(&self, value: &Value, ty: &TypeRef, path: &str) -> NativeResult<()> {
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
                _ => Err(NativeError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!(
                        "expected Result, got {}",
                        self.value_type_name(&value)
                    ),
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
                        return Err(NativeError::Runtime(
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
                        return Err(NativeError::Runtime(
                            "Result expects 2 type arguments".to_string(),
                        ));
                    }
                    match value {
                        Value::ResultOk(inner) => self.validate_value(&inner, &args[0], path),
                        Value::ResultErr(inner) => self.validate_value(&inner, &args[1], path),
                        _ => Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected Result, got {}",
                                self.value_type_name(&value)
                            ),
                        ))),
                    }
                }
                "List" => {
                    if args.len() != 1 {
                        return Err(NativeError::Runtime(
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
                        _ => Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected List, got {}",
                                self.value_type_name(&value)
                            ),
                        ))),
                    }
                }
                "Map" => {
                    if args.len() != 2 {
                        return Err(NativeError::Runtime(
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
                        _ => Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected Map, got {}",
                                self.value_type_name(&value)
                            ),
                        ))),
                    }
                }
                _ => Err(NativeError::Runtime(format!(
                    "validation not supported for {}",
                    base.name
                ))),
            },
        }
    }

    fn validate_simple(&self, value: &Value, name: &str, path: &str) -> NativeResult<()> {
        let value = value.unboxed();
        let type_name = self.value_type_name(&value);
        let (module, simple_name) = split_type_name(name);
        if module.is_none() {
            match simple_name {
                "Int" => {
                    if matches!(value, Value::Int(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Int, got {type_name}"),
                    )));
                }
                "Float" => {
                    if matches!(value, Value::Float(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Float, got {type_name}"),
                    )));
                }
                "Bool" => {
                    if matches!(value, Value::Bool(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bool, got {type_name}"),
                    )));
                }
                "String" => {
                    if matches!(value, Value::String(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected String, got {type_name}"),
                    )));
                }
                "Id" => match value {
                    Value::String(s) if !s.is_empty() => return Ok(()),
                    Value::String(_) => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "expected non-empty Id".to_string(),
                        )))
                    }
                    _ => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Id, got {type_name}"),
                        )))
                    }
                },
                "Email" => match value {
                    Value::String(s) if rt_validate::is_email(&s) => return Ok(()),
                    Value::String(_) => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "invalid email address".to_string(),
                        )))
                    }
                    _ => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Email, got {type_name}"),
                        )))
                    }
                },
                "Bytes" => {
                    if matches!(value, Value::String(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bytes, got {type_name}"),
                    )));
                }
                _ => {}
            }
        }
        match value {
            Value::Struct { name: struct_name, .. } if struct_name == simple_name => Ok(()),
            Value::Enum { name: enum_name, .. } if enum_name == simple_name => Ok(()),
            _ => Err(NativeError::Error(self.validation_error_value(
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
            Value::Null => "Null".to_string(),
            Value::List(_) => "List".to_string(),
            Value::Map(_) => "Map".to_string(),
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

    fn is_optional_type(&self, ty: &TypeRef) -> bool {
        match &ty.kind {
            TypeRefKind::Optional(_) => true,
            TypeRefKind::Generic { base, .. } => base.name == "Option",
            _ => false,
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

    fn check_refined(
        &self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> NativeResult<()> {
        let value = value.unboxed();
        match base {
            "String" => {
                let (min, max) = self.parse_length_range(args)?;
                let len = match value {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined String expects a String".to_string(),
                        ))
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
                let (min, max) = self.parse_int_range(args)?;
                let val = match value {
                    Value::Int(v) => v,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Int expects an Int".to_string(),
                        ))
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
                let (min, max) = self.parse_float_range(args)?;
                let val = match value {
                    Value::Float(v) => v,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Float expects a Float".to_string(),
                        ))
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
            _ => Ok(()),
        }
    }

    fn parse_length_range(&self, args: &[Expr]) -> NativeResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_int_range(&self, args: &[Expr]) -> NativeResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_float_range(&self, args: &[Expr]) -> NativeResult<(f64, f64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_f64(left)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_f64(right)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn extract_range_args<'b>(&self, args: &'b [Expr]) -> NativeResult<(&'b Expr, &'b Expr)> {
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
        Err(NativeError::Runtime(
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

    fn value_to_json(&self, value: &Value) -> rt_json::JsonValue {
        match value.unboxed() {
            Value::Unit => rt_json::JsonValue::Null,
            Value::Int(v) => rt_json::JsonValue::Number(v as f64),
            Value::Float(v) => rt_json::JsonValue::Number(v),
            Value::Bool(v) => rt_json::JsonValue::Bool(v),
            Value::String(v) => rt_json::JsonValue::String(v.clone()),
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

    fn decode_json_value(
        &mut self,
        json: &rt_json::JsonValue,
        ty: &TypeRef,
        path: &str,
    ) -> NativeResult<Value> {
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
                    } else if self.program.ir.types.contains_key(simple_name) {
                        self.decode_struct_json(json, simple_name, path)?
                    } else if self.program.ir.enums.contains_key(simple_name) {
                        self.decode_enum_json(json, simple_name, path)?
                    } else {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("unknown type {}", ident.name),
                        )));
                    }
                } else if self.program.ir.types.contains_key(simple_name) {
                    self.decode_struct_json(json, simple_name, path)?
                } else if self.program.ir.enums.contains_key(simple_name) {
                    self.decode_enum_json(json, simple_name, path)?
                } else {
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("unknown type {}", ident.name),
                    )));
                }
            }
            TypeRefKind::Result { .. } => {
                return Err(NativeError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    "Result is not supported for JSON body",
                )))
            }
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(NativeError::Runtime(
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
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "invalid_value",
                        "Result is not supported for JSON body",
                    )))
                }
                "List" => {
                    if args.len() != 1 {
                        return Err(NativeError::Runtime(
                            "List expects 1 type argument".to_string(),
                        ));
                    }
                    let rt_json::JsonValue::Array(items) = json else {
                        return Err(NativeError::Error(self.validation_error_value(
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
                        return Err(NativeError::Runtime(
                            "Map expects 2 type arguments".to_string(),
                        ));
                    }
                    let rt_json::JsonValue::Object(items) = json else {
                        return Err(NativeError::Error(self.validation_error_value(
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
                    return Err(NativeError::Error(self.validation_error_value(
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
    ) -> NativeResult<Option<Value>> {
        let value = match name {
            "Int" => match json {
                rt_json::JsonValue::Number(n) if n.fract() == 0.0 => Value::Int(*n as i64),
                _ => {
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Int",
                    )))
                }
            },
            "Float" => match json {
                rt_json::JsonValue::Number(n) => Value::Float(*n),
                _ => {
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Float",
                    )))
                }
            },
            "Bool" => match json {
                rt_json::JsonValue::Bool(v) => Value::Bool(*v),
                _ => {
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        "expected Bool",
                    )))
                }
            },
            "String" | "Id" | "Email" | "Bytes" => match json {
                rt_json::JsonValue::String(v) => Value::String(v.clone()),
                _ => {
                    return Err(NativeError::Error(self.validation_error_value(
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
                vec![self.decode_json_value(
                    data,
                    &variant.payload[0],
                    &format!("{path}.data"),
                )?]
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
    if args.len() != func.params.len() {
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
                if is_result_type(func.ret.as_ref()) {
                    Ok(Value::ResultErr(Box::new(err_val)))
                } else {
                    Err(format_error_value(&err_val))
                }
            }
            Err(JitCallError::Runtime(message)) => Err(message),
        };
        heap.collect_garbage();
        return out;
    }
    heap.collect_garbage();
    let reason = unsupported_reason(&program.ir, func)
        .unwrap_or_else(|| "native backend could not compile function".to_string());
    Err(format!("native backend unsupported: {reason}"))
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

fn unsupported_reason(program: &IrProgram, func: &Function) -> Option<String> {
    let mut seen = HashSet::new();
    unsupported_reason_inner(program, func, &mut seen)
}

fn unsupported_reason_inner(
    program: &IrProgram,
    func: &Function,
    seen: &mut HashSet<String>,
) -> Option<String> {
    if !seen.insert(func.name.clone()) {
        return None;
    }
    let func_name = func.name.as_str();
    for instr in &func.code {
        match instr {
            crate::ir::Instr::Spawn { .. } | crate::ir::Instr::Await => {
                return Some(format!("spawn/await in {func_name}"));
            }
            crate::ir::Instr::SetField { .. } => {
                return Some(format!("field assignment in {func_name}"));
            }
            crate::ir::Instr::SetIndex => {
                return Some(format!("index assignment in {func_name}"));
            }
            crate::ir::Instr::GetIndex => {
                return Some(format!("index access in {func_name}"));
            }
            crate::ir::Instr::IterInit | crate::ir::Instr::IterNext { .. } => {
                return Some(format!("iteration in {func_name}"));
            }
            crate::ir::Instr::Call {
                kind: crate::ir::CallKind::Function,
                name,
                ..
            } => {
                if let Some(callee) = program
                    .functions
                    .get(name)
                    .or_else(|| program.apps.get(name))
                    .or_else(|| program.apps.values().find(|func| func.name == name.as_str()))
                {
                    if let Some(reason) = unsupported_reason_inner(program, callee, seen) {
                        return Some(reason);
                    }
                }
            }
            crate::ir::Instr::Call {
                kind: crate::ir::CallKind::Builtin,
                name,
                ..
            } => {
                if !is_supported_builtin(name) {
                    return Some(format!("builtin {name} in {func_name}"));
                }
            }
            _ => {}
        }
    }
    None
}

fn is_supported_builtin(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "log"
            | "env"
            | "assert"
            | "task.id"
            | "task.done"
            | "task.cancel"
            | "db.exec"
            | "db.query"
            | "db.one"
            | "json.encode"
            | "json.decode"
    )
}

struct ConfigEvaluator;

impl ConfigEvaluator {
    fn eval_configs<'a, I>(
        &self,
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

    fn parse_env_value(&self, ty: &TypeRef, raw: &str) -> NativeResult<Value> {
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
            TypeRefKind::Result { .. } => Err(NativeError::Runtime(
                "Result is not supported for config env overrides".to_string(),
            )),
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "Option" => {
                    if args.len() != 1 {
                        return Err(NativeError::Runtime(
                            "Option expects 1 type argument".to_string(),
                        ));
                    }
                    if raw.eq_ignore_ascii_case("null") || raw.is_empty() {
                        Ok(Value::Null)
                    } else {
                        self.parse_env_value(&args[0], raw)
                    }
                }
                "Result" => Err(NativeError::Runtime(
                    "Result is not supported for config env overrides".to_string(),
                )),
                _ => Err(NativeError::Runtime(
                    "config env overrides only support simple types".to_string(),
                )),
            },
        }
    }

    fn parse_simple_env(&self, name: &str, raw: &str) -> NativeResult<Value> {
        match name {
            "Int" => raw
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| NativeError::Runtime(format!("invalid Int: {raw}"))),
            "Float" => raw
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| NativeError::Runtime(format!("invalid Float: {raw}"))),
            "Bool" => match raw.to_ascii_lowercase().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(NativeError::Runtime(format!("invalid Bool: {raw}"))),
            },
            "String" | "Id" | "Email" | "Bytes" => Ok(Value::String(raw.to_string())),
            _ => Err(NativeError::Runtime(format!(
                "env override not supported for type {name}"
            ))),
        }
    }

    fn map_parse_error(&self, err: NativeError, path: &str) -> NativeError {
        match err {
            NativeError::Runtime(message) => {
                NativeError::Error(self.validation_error_value(path, "invalid_value", message))
            }
            other => other,
        }
    }

    fn validate_value(&self, value: &Value, ty: &TypeRef, path: &str) -> NativeResult<()> {
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
                _ => Err(NativeError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!(
                        "expected Result, got {}",
                        self.value_type_name(&value)
                    ),
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
                        return Err(NativeError::Runtime(
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
                        return Err(NativeError::Runtime(
                            "Result expects 2 type arguments".to_string(),
                        ));
                    }
                    match value {
                        Value::ResultOk(inner) => self.validate_value(&inner, &args[0], path),
                        Value::ResultErr(inner) => self.validate_value(&inner, &args[1], path),
                        _ => Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected Result, got {}",
                                self.value_type_name(&value)
                            ),
                        ))),
                    }
                }
                "List" => {
                    if args.len() != 1 {
                        return Err(NativeError::Runtime(
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
                        _ => Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected List, got {}",
                                self.value_type_name(&value)
                            ),
                        ))),
                    }
                }
                "Map" => {
                    if args.len() != 2 {
                        return Err(NativeError::Runtime(
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
                        _ => Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected Map, got {}",
                                self.value_type_name(&value)
                            ),
                        ))),
                    }
                }
                _ => Err(NativeError::Runtime(format!(
                    "validation not supported for {}",
                    base.name
                ))),
            },
        }
    }

    fn validate_simple(&self, value: &Value, name: &str, path: &str) -> NativeResult<()> {
        let value = value.unboxed();
        let type_name = self.value_type_name(&value);
        let (module, simple_name) = split_type_name(name);
        if module.is_none() {
            match simple_name {
                "Int" => {
                    if matches!(value, Value::Int(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Int, got {type_name}"),
                    )));
                }
                "Float" => {
                    if matches!(value, Value::Float(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Float, got {type_name}"),
                    )));
                }
                "Bool" => {
                    if matches!(value, Value::Bool(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bool, got {type_name}"),
                    )));
                }
                "String" => {
                    if matches!(value, Value::String(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected String, got {type_name}"),
                    )));
                }
                "Id" => match value {
                    Value::String(s) if !s.is_empty() => return Ok(()),
                    Value::String(_) => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "expected non-empty Id".to_string(),
                        )))
                    }
                    _ => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Id, got {type_name}"),
                        )))
                    }
                },
                "Email" => match value {
                    Value::String(s) if rt_validate::is_email(&s) => return Ok(()),
                    Value::String(_) => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "invalid_value",
                            "invalid email address".to_string(),
                        )))
                    }
                    _ => {
                        return Err(NativeError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!("expected Email, got {type_name}"),
                        )))
                    }
                },
                "Bytes" => {
                    if matches!(value, Value::String(_)) {
                        return Ok(());
                    }
                    return Err(NativeError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bytes, got {type_name}"),
                    )));
                }
                _ => {}
            }
        }
        match value {
            Value::Struct { name: struct_name, .. } if struct_name == simple_name => Ok(()),
            Value::Enum { name: enum_name, .. } if enum_name == simple_name => Ok(()),
            _ => Err(NativeError::Error(self.validation_error_value(
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
            Value::Null => "Null".to_string(),
            Value::List(_) => "List".to_string(),
            Value::Map(_) => "Map".to_string(),
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

    fn check_refined(
        &self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> NativeResult<()> {
        let value = value.unboxed();
        match base {
            "String" => {
                let (min, max) = self.parse_length_range(args)?;
                let len = match value {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined String expects a String".to_string(),
                        ))
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
                let (min, max) = self.parse_int_range(args)?;
                let val = match value {
                    Value::Int(v) => v,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Int expects an Int".to_string(),
                        ))
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
                let (min, max) = self.parse_float_range(args)?;
                let val = match value {
                    Value::Float(v) => v,
                    _ => {
                        return Err(NativeError::Runtime(
                            "refined Float expects a Float".to_string(),
                        ))
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
            _ => Ok(()),
        }
    }

    fn parse_length_range(&self, args: &[Expr]) -> NativeResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_int_range(&self, args: &[Expr]) -> NativeResult<(i64, i64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_i64(left)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_i64(right)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn parse_float_range(&self, args: &[Expr]) -> NativeResult<(f64, f64)> {
        let (left, right) = self.extract_range_args(args)?;
        let min = self
            .literal_to_f64(left)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        let max = self
            .literal_to_f64(right)
            .ok_or_else(|| NativeError::Runtime("invalid refined range".to_string()))?;
        Ok((min, max))
    }

    fn extract_range_args<'b>(&self, args: &'b [Expr]) -> NativeResult<(&'b Expr, &'b Expr)> {
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
        Err(NativeError::Runtime(
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
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
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
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
}
