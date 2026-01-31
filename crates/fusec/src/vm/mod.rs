use std::collections::HashMap;

use fuse_rt::{config as rt_config, validate as rt_validate};

use crate::ast::{Expr, ExprKind, Literal, Pattern, PatternKind, TypeRef, TypeRefKind, UnaryOp};
use crate::interp::{format_error_value, Value};
use crate::ir::{CallKind, Const, Function, Instr, Program as IrProgram};

#[derive(Debug)]
enum VmError {
    Runtime(String),
    Error(Value),
}

type VmResult<T> = Result<T, VmError>;

pub struct Vm<'a> {
    program: &'a IrProgram,
    configs: HashMap<String, HashMap<String, Value>>,
}

impl<'a> Vm<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self {
            program,
            configs: HashMap::new(),
        }
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

    fn render_error(&self, err: VmError) -> String {
        match err {
            VmError::Runtime(msg) => msg,
            VmError::Error(value) => format_error_value(&value),
        }
    }

    fn eval_configs(&mut self) -> VmResult<()> {
        let config_path =
            std::env::var("FUSE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        let file_values =
            rt_config::load_config_file(&config_path).map_err(VmError::Runtime)?;
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
                                let func = self
                                    .program
                                    .functions
                                    .get(fn_name)
                                    .ok_or_else(|| {
                                        VmError::Runtime(format!(
                                            "unknown config default {fn_name}"
                                        ))
                                    })?;
                                self.exec_function(func, Vec::new())?
                            } else {
                                Value::Null
                            }
                        } else if let Some(fn_name) = &field.default_fn {
                            let func = self
                                .program
                                .functions
                                .get(fn_name)
                                .ok_or_else(|| {
                                    VmError::Runtime(format!(
                                        "unknown config default {fn_name}"
                                    ))
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
                    frame.locals[slot] = value;
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
                Instr::Eq
                | Instr::NotEq
                | Instr::Lt
                | Instr::LtEq
                | Instr::Gt
                | Instr::GtEq => {
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
                    if matches!(value, Value::Null) {
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
                            let func = self
                                .program
                                .functions
                                .get(&name)
                                .ok_or_else(|| VmError::Runtime(format!("unknown function {name}")))?;
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
                Instr::Return => {
                    let value = if frame.stack.is_empty() {
                        Value::Unit
                    } else {
                        frame.pop()?
                    };
                    return Ok(self.wrap_function_result(func, value)?);
                }
                Instr::Bang { has_error } => {
                    let err_value = if has_error {
                        Some(frame.pop()?)
                    } else {
                        None
                    };
                    let value = frame.pop()?;
                    match value {
                        Value::Null => {
                            let err = err_value.unwrap_or_else(|| self.default_error_value("missing value"));
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
                            return Err(VmError::Runtime(format!(
                                "?! expects Option or Result, got {}",
                                self.value_type_name(&other)
                            )));
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
                                )))
                            }
                        };
                        map.insert(key, value);
                    }
                    frame.stack.push(Value::Map(map));
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
                Instr::MakeEnum { name, variant, argc } => {
                    let mut payload = Vec::with_capacity(argc);
                    for _ in 0..argc {
                        payload.push(frame.pop()?);
                    }
                    payload.reverse();
                    let value = self.make_enum(&name, &variant, payload)?;
                    frame.stack.push(value);
                }
                Instr::GetField { field } => {
                    let base = frame.pop()?;
                    let value = match base {
                        Value::Struct { fields, .. } => fields.get(&field).cloned().ok_or_else(|| {
                            VmError::Runtime(format!("unknown field {field}"))
                        })?,
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
                            ))
                        }
                    };
                    frame.stack.push(value);
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
                    let value = map
                        .get(&field)
                        .cloned()
                        .ok_or_else(|| VmError::Runtime(format!("unknown config field {config}.{field}")))?;
                    frame.stack.push(value);
                }
            }
        }
    }

    fn eval_builtin(&mut self, name: &str, args: Vec<Value>) -> VmResult<Value> {
        match name {
            "print" => {
                let text = args.get(0).map(|v| v.to_string_value()).unwrap_or_default();
                println!("{text}");
                Ok(Value::Unit)
            }
            "env" => {
                let key = match args.get(0) {
                    Some(Value::String(s)) => s.clone(),
                    _ => return Err(VmError::Runtime("env expects a string argument".to_string())),
                };
                match std::env::var(key) {
                    Ok(value) => Ok(Value::String(value)),
                    Err(_) => Ok(Value::Null),
                }
            }
            "serve" => Err(VmError::Runtime(
                "serve is not supported in the VM yet".to_string(),
            )),
            _ => Err(VmError::Runtime(format!("unknown builtin {name}"))),
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
            "String" | "Id" | "Email" | "Bytes" => Ok(Value::String(raw.to_string())),
            _ => Err(VmError::Runtime(format!(
                "env override not supported for type {name}"
            ))),
        }
    }

    fn validate_value(&self, value: &Value, ty: &TypeRef, path: &str) -> VmResult<()> {
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
                _ => Err(VmError::Error(self.validation_error_value(
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
                        return Err(VmError::Runtime(
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
                        return Err(VmError::Runtime(
                            "Result expects 2 type arguments".to_string(),
                        ));
                    }
                    match value {
                        Value::ResultOk(inner) => self.validate_value(inner, &args[0], path),
                        Value::ResultErr(inner) => self.validate_value(inner, &args[1], path),
                        _ => Err(VmError::Error(self.validation_error_value(
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
                        return Err(VmError::Runtime(
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
                        _ => Err(VmError::Error(self.validation_error_value(
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
                        return Err(VmError::Runtime(
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
                        _ => Err(VmError::Error(self.validation_error_value(
                            path,
                            "type_mismatch",
                            format!(
                                "expected Map, got {}",
                                self.value_type_name(value)
                            ),
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
        let type_name = self.value_type_name(value);
        match name {
            "Int" => {
                if matches!(value, Value::Int(_)) {
                    Ok(())
                } else {
                    Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Int, got {type_name}"),
                    )))
                }
            }
            "Float" => {
                if matches!(value, Value::Float(_)) {
                    Ok(())
                } else {
                    Err(VmError::Error(self.validation_error_value(
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
                    Err(VmError::Error(self.validation_error_value(
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
                    Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected String, got {type_name}"),
                    )))
                }
            }
            "Id" => match value {
                Value::String(s) if !s.is_empty() => Ok(()),
                Value::String(_) => Err(VmError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    "expected non-empty Id".to_string(),
                ))),
                _ => Err(VmError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Id, got {type_name}"),
                ))),
            },
            "Email" => match value {
                Value::String(s) if rt_validate::is_email(s) => Ok(()),
                Value::String(_) => Err(VmError::Error(self.validation_error_value(
                    path,
                    "invalid_value",
                    "invalid email address".to_string(),
                ))),
                _ => Err(VmError::Error(self.validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Email, got {type_name}"),
                ))),
            },
            "Bytes" => {
                if matches!(value, Value::String(_)) {
                    Ok(())
                } else {
                    Err(VmError::Error(self.validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Bytes, got {type_name}"),
                    )))
                }
            }
            _ => match value {
                Value::Struct { name: struct_name, .. } if struct_name == name => Ok(()),
                Value::Enum { name: enum_name, .. } if enum_name == name => Ok(()),
                _ => Err(VmError::Error(self.validation_error_value(
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
            name: "Error".to_string(),
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
        if variant_info.arity != payload.len() {
            return Err(VmError::Runtime(format!(
                "variant {name}.{variant} expects {} value(s), got {}",
                variant_info.arity,
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
        match &pat.kind {
            PatternKind::Wildcard => Ok(true),
            PatternKind::Literal(lit) => Ok(self.literal_matches(value, lit)),
            PatternKind::Ident(ident) => {
                if let Some(is_match) = self.match_variant_ident(value, &ident.name) {
                    return Ok(is_match);
                }
                if let Value::Enum { name, .. } = value {
                    if self.enum_variant_exists(name, &ident.name) {
                        return Ok(false);
                    }
                }
                bindings.insert(ident.name.clone(), value.clone());
                Ok(true)
            }
            PatternKind::EnumVariant { name, args } => {
                self.match_enum_variant(value, &name.name, args, bindings)
            }
            PatternKind::Struct { name, fields } => {
                self.match_struct_pattern(value, &name.name, fields, bindings)
            }
        }
    }

    fn match_variant_ident(&self, value: &Value, name: &str) -> Option<bool> {
        match name {
            "None" => Some(matches!(value, Value::Null)),
            "Some" => Some(!matches!(value, Value::Null)),
            "Ok" => Some(matches!(value, Value::ResultOk(_))),
            "Err" => Some(matches!(value, Value::ResultErr(_))),
            _ => match value {
                Value::Enum { name: enum_name, variant, payload } => {
                    if variant == name {
                        let arity = self.enum_variant_arity(enum_name, variant).unwrap_or(0);
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
        match name {
            "Some" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("Some expects 1 pattern".to_string()));
                }
                if matches!(value, Value::Null) {
                    return Ok(false);
                }
                self.match_pattern(value, &args[0], bindings)
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
                    Value::ResultOk(inner) => self.match_pattern(inner, &args[0], bindings),
                    _ => Ok(false),
                }
            }
            "Err" => {
                if args.len() != 1 {
                    return Err(VmError::Runtime("Err expects 1 pattern".to_string()));
                }
                match value {
                    Value::ResultErr(inner) => self.match_pattern(inner, &args[0], bindings),
                    _ => Ok(false),
                }
            }
            _ => match value {
                Value::Enum { name: enum_name, variant, payload } => {
                    if variant != name {
                        return Ok(false);
                    }
                    let arity = self.enum_variant_arity(enum_name, variant).unwrap_or(payload.len());
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
                .map(|v| v.arity)
        })
    }

    fn enum_variant_exists(&self, enum_name: &str, variant: &str) -> bool {
        self.enum_variant_arity(enum_name, variant).is_some()
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

    fn check_refined(
        &self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> VmResult<()> {
        match base {
            "String" => {
                let (min, max) = self.parse_length_range(args)?;
                let len = match value {
                    Value::String(s) => s.chars().count() as i64,
                    _ => {
                        return Err(VmError::Runtime(
                            "refined String expects a String".to_string(),
                        ))
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
                    Value::Int(v) => *v,
                    _ => {
                        return Err(VmError::Runtime(
                            "refined Int expects an Int".to_string(),
                        ))
                    }
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
                    Value::Float(v) => *v,
                    _ => {
                        return Err(VmError::Runtime(
                            "refined Float expects a Float".to_string(),
                        ))
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
                Instr::Mod => Err(VmError::Runtime(
                    "mod not supported for float".to_string(),
                )),
                _ => Err(VmError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Int(a), Value::Float(b)) => match op {
                Instr::Sub => Ok(Value::Float(a as f64 - b)),
                Instr::Mul => Ok(Value::Float(a as f64 * b)),
                Instr::Div => Ok(Value::Float(a as f64 / b)),
                Instr::Mod => Err(VmError::Runtime(
                    "mod not supported for float".to_string(),
                )),
                _ => Err(VmError::Runtime("unsupported arithmetic op".to_string())),
            },
            (Value::Float(a), Value::Int(b)) => match op {
                Instr::Sub => Ok(Value::Float(a - b as f64)),
                Instr::Mul => Ok(Value::Float(a * b as f64)),
                Instr::Div => Ok(Value::Float(a / b as f64)),
                Instr::Mod => Err(VmError::Runtime(
                    "mod not supported for float".to_string(),
                )),
                _ => Err(VmError::Runtime("unsupported arithmetic op".to_string())),
            },
            _ => Err(VmError::Runtime(
                "unsupported arithmetic operands".to_string(),
            )),
        }
    }

    fn eval_compare(&self, op: &Instr, left: Value, right: Value) -> VmResult<Value> {
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
                    ))
                }
            },
            _ => {
                return Err(VmError::Runtime(
                    "unsupported comparison operands".to_string(),
                ))
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
        match value {
            Value::Bool(v) => Ok(*v),
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
