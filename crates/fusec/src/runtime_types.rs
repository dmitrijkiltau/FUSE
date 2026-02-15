use std::collections::{BTreeMap, HashMap};

use fuse_rt::{bytes as rt_bytes, json as rt_json, validate as rt_validate};

use crate::ast::{Expr, TypeRef, TypeRefKind};
use crate::interp::Value;

pub(crate) trait RuntimeTypeHost {
    type Error;

    fn runtime_error(&self, message: String) -> Self::Error;
    fn validation_error(&self, path: &str, code: &str, message: String) -> Self::Error;

    fn has_struct_type(&self, name: &str) -> bool;
    fn has_enum_type(&self, name: &str) -> bool;

    fn decode_struct_type_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> Result<Value, Self::Error>;

    fn decode_enum_type_json(
        &mut self,
        json: &rt_json::JsonValue,
        name: &str,
        path: &str,
    ) -> Result<Value, Self::Error>;

    fn check_refined_value(
        &mut self,
        value: &Value,
        base: &str,
        args: &[Expr],
        path: &str,
    ) -> Result<(), Self::Error>;
}

pub fn split_type_name(name: &str) -> (Option<&str>, &str) {
    if name.starts_with("std.") {
        return (None, name);
    }
    match name.split_once('.') {
        Some((module, rest)) if !module.is_empty() && !rest.is_empty() => (Some(module), rest),
        _ => (None, name),
    }
}

pub fn value_type_name(value: &Value) -> String {
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

pub fn is_optional_type(ty: &TypeRef) -> bool {
    match &ty.kind {
        TypeRefKind::Optional(_) => true,
        TypeRefKind::Generic { base, .. } => base.name == "Option",
        _ => false,
    }
}

pub fn parse_simple_env(name: &str, raw: &str) -> Result<Value, String> {
    match name {
        "Int" => raw
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("invalid Int: {raw}")),
        "Float" => raw
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("invalid Float: {raw}")),
        "Bool" => match raw.to_ascii_lowercase().as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("invalid Bool: {raw}")),
        },
        "String" | "Id" | "Email" => Ok(Value::String(raw.to_string())),
        "Bytes" => {
            let bytes = rt_bytes::decode_base64(raw)
                .map_err(|msg| format!("invalid Bytes (base64): {msg}"))?;
            Ok(Value::Bytes(bytes))
        }
        _ => Err(format!("env override not supported for type {name}")),
    }
}

pub(crate) fn parse_env_value<H: RuntimeTypeHost>(
    host: &mut H,
    ty: &TypeRef,
    raw: &str,
) -> Result<Value, H::Error> {
    let raw = raw.trim();
    match &ty.kind {
        TypeRefKind::Optional(inner) => {
            if raw.eq_ignore_ascii_case("null") || raw.is_empty() {
                Ok(Value::Null)
            } else {
                parse_env_value(host, inner, raw)
            }
        }
        TypeRefKind::Refined { base, .. } => {
            let (_, simple_name) = split_type_name(&base.name);
            parse_simple_env(simple_name, raw).map_err(|msg| host.runtime_error(msg))
        }
        TypeRefKind::Simple(ident) => {
            let (_, simple_name) = split_type_name(&ident.name);
            match simple_name {
                "Int" | "Float" | "Bool" | "String" | "Id" | "Email" | "Bytes" | "Html" => {
                    parse_simple_env(simple_name, raw).map_err(|msg| host.runtime_error(msg))
                }
                _ => {
                    let json = rt_json::decode(raw)
                        .map_err(|msg| host.runtime_error(format!("invalid JSON value: {msg}")))?;
                    decode_json_value(host, &json, ty, "$")
                }
            }
        }
        TypeRefKind::Result { .. } => {
            Err(host.runtime_error("Result is not supported for config env overrides".to_string()))
        }
        TypeRefKind::Generic { base, args } => match base.name.as_str() {
            "Option" => {
                if args.len() != 1 {
                    return Err(host.runtime_error("Option expects 1 type argument".to_string()));
                }
                if raw.eq_ignore_ascii_case("null") || raw.is_empty() {
                    Ok(Value::Null)
                } else {
                    parse_env_value(host, &args[0], raw)
                }
            }
            "Result" => {
                Err(host
                    .runtime_error("Result is not supported for config env overrides".to_string()))
            }
            _ => {
                let json = rt_json::decode(raw)
                    .map_err(|msg| host.runtime_error(format!("invalid JSON value: {msg}")))?;
                decode_json_value(host, &json, ty, "$")
            }
        },
    }
}

pub(crate) fn validate_value<H: RuntimeTypeHost>(
    host: &mut H,
    value: &Value,
    ty: &TypeRef,
    path: &str,
) -> Result<(), H::Error> {
    let value = value.unboxed();
    match &ty.kind {
        TypeRefKind::Optional(inner) => {
            if matches!(value, Value::Null) {
                Ok(())
            } else {
                validate_value(host, &value, inner, path)
            }
        }
        TypeRefKind::Result { ok, err } => match value {
            Value::ResultOk(inner) => validate_value(host, &inner, ok, path),
            Value::ResultErr(inner) => {
                if let Some(err_ty) = err {
                    validate_value(host, &inner, err_ty, path)
                } else {
                    Ok(())
                }
            }
            _ => Err(host.validation_error(
                path,
                "type_mismatch",
                format!("expected Result, got {}", value_type_name(&value)),
            )),
        },
        TypeRefKind::Refined { base, args } => {
            validate_simple_value(host, &value, &base.name, path)?;
            host.check_refined_value(&value, &base.name, args, path)
        }
        TypeRefKind::Simple(ident) => validate_simple_value(host, &value, &ident.name, path),
        TypeRefKind::Generic { base, args } => match base.name.as_str() {
            "Option" => {
                if args.len() != 1 {
                    return Err(host.runtime_error("Option expects 1 type argument".to_string()));
                }
                if matches!(value, Value::Null) {
                    Ok(())
                } else {
                    validate_value(host, &value, &args[0], path)
                }
            }
            "Result" => {
                if args.len() != 2 {
                    return Err(host.runtime_error("Result expects 2 type arguments".to_string()));
                }
                match value {
                    Value::ResultOk(inner) => validate_value(host, &inner, &args[0], path),
                    Value::ResultErr(inner) => validate_value(host, &inner, &args[1], path),
                    _ => Err(host.validation_error(
                        path,
                        "type_mismatch",
                        format!("expected Result, got {}", value_type_name(&value)),
                    )),
                }
            }
            "List" => {
                if args.len() != 1 {
                    return Err(host.runtime_error("List expects 1 type argument".to_string()));
                }
                match value {
                    Value::List(items) => {
                        for (idx, item) in items.iter().enumerate() {
                            let item_path = format!("{path}[{idx}]");
                            validate_value(host, item, &args[0], &item_path)?;
                        }
                        Ok(())
                    }
                    _ => Err(host.validation_error(
                        path,
                        "type_mismatch",
                        format!("expected List, got {}", value_type_name(&value)),
                    )),
                }
            }
            "Map" => {
                if args.len() != 2 {
                    return Err(host.runtime_error("Map expects 2 type arguments".to_string()));
                }
                match value {
                    Value::Map(items) => {
                        for (key, val) in items.iter() {
                            let key_value = Value::String(key.clone());
                            let key_path = format!("{path}.{key}");
                            validate_value(host, &key_value, &args[0], &key_path)?;
                            validate_value(host, val, &args[1], &key_path)?;
                        }
                        Ok(())
                    }
                    _ => Err(host.validation_error(
                        path,
                        "type_mismatch",
                        format!("expected Map, got {}", value_type_name(&value)),
                    )),
                }
            }
            _ => Err(host.runtime_error(format!("validation not supported for {}", base.name))),
        },
    }
}

pub(crate) fn decode_json_value<H: RuntimeTypeHost>(
    host: &mut H,
    json: &rt_json::JsonValue,
    ty: &TypeRef,
    path: &str,
) -> Result<Value, H::Error> {
    let value = match &ty.kind {
        TypeRefKind::Optional(inner) => {
            if matches!(json, rt_json::JsonValue::Null) {
                Value::Null
            } else {
                decode_json_value(host, json, inner, path)?
            }
        }
        TypeRefKind::Refined { base, .. } => {
            let base_ty = TypeRef {
                kind: TypeRefKind::Simple(base.clone()),
                span: ty.span,
            };
            let value = decode_json_value(host, json, &base_ty, path)?;
            validate_value(host, &value, ty, path)?;
            return Ok(value);
        }
        TypeRefKind::Simple(ident) => {
            let (module, simple_name) = split_type_name(&ident.name);
            if module.is_none() {
                if let Some(value) = decode_simple_json(host, json, simple_name, path)? {
                    value
                } else if host.has_struct_type(simple_name) {
                    host.decode_struct_type_json(json, simple_name, path)?
                } else if host.has_enum_type(simple_name) {
                    host.decode_enum_type_json(json, simple_name, path)?
                } else {
                    return Err(host.validation_error(
                        path,
                        "type_mismatch",
                        format!("unknown type {}", ident.name),
                    ));
                }
            } else if host.has_struct_type(simple_name) {
                host.decode_struct_type_json(json, simple_name, path)?
            } else if host.has_enum_type(simple_name) {
                host.decode_enum_type_json(json, simple_name, path)?
            } else {
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("unknown type {}", ident.name),
                ));
            }
        }
        TypeRefKind::Result { ok, err } => {
            return decode_json_result_value(host, json, ok, err.as_deref(), path);
        }
        TypeRefKind::Generic { base, args } => match base.name.as_str() {
            "Option" => {
                if args.len() != 1 {
                    return Err(host.runtime_error("Option expects 1 type argument".to_string()));
                }
                if matches!(json, rt_json::JsonValue::Null) {
                    Value::Null
                } else {
                    decode_json_value(host, json, &args[0], path)?
                }
            }
            "Result" => {
                if args.len() != 2 {
                    return Err(host.runtime_error("Result expects 2 type arguments".to_string()));
                }
                return decode_json_result_value(host, json, &args[0], Some(&args[1]), path);
            }
            "List" => {
                if args.len() != 1 {
                    return Err(host.runtime_error("List expects 1 type argument".to_string()));
                }
                let rt_json::JsonValue::Array(items) = json else {
                    return Err(host.validation_error(
                        path,
                        "type_mismatch",
                        "expected List".to_string(),
                    ));
                };
                let mut values = Vec::with_capacity(items.len());
                for (idx, item) in items.iter().enumerate() {
                    let item_path = format!("{path}[{idx}]");
                    values.push(decode_json_value(host, item, &args[0], &item_path)?);
                }
                Value::List(values)
            }
            "Map" => {
                if args.len() != 2 {
                    return Err(host.runtime_error("Map expects 2 type arguments".to_string()));
                }
                let rt_json::JsonValue::Object(items) = json else {
                    return Err(host.validation_error(
                        path,
                        "type_mismatch",
                        "expected Map".to_string(),
                    ));
                };
                let mut values = HashMap::new();
                for (key, item) in items.iter() {
                    let key_value = Value::String(key.clone());
                    let key_path = format!("{path}.{key}");
                    validate_value(host, &key_value, &args[0], &key_path)?;
                    let value = decode_json_value(host, item, &args[1], &key_path)?;
                    values.insert(key.clone(), value);
                }
                Value::Map(values)
            }
            _ => {
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("unsupported type {}", base.name),
                ));
            }
        },
    };
    validate_value(host, &value, ty, path)?;
    Ok(value)
}

fn validate_simple_value<H: RuntimeTypeHost>(
    host: &H,
    value: &Value,
    name: &str,
    path: &str,
) -> Result<(), H::Error> {
    let value = value.unboxed();
    let type_name = value_type_name(&value);
    let (module, simple_name) = split_type_name(name);
    if module.is_none() {
        match simple_name {
            "Int" => {
                if matches!(value, Value::Int(_)) {
                    return Ok(());
                }
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("expected Int, got {type_name}"),
                ));
            }
            "Float" => {
                if matches!(value, Value::Float(_)) {
                    return Ok(());
                }
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("expected Float, got {type_name}"),
                ));
            }
            "Bool" => {
                if matches!(value, Value::Bool(_)) {
                    return Ok(());
                }
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("expected Bool, got {type_name}"),
                ));
            }
            "String" => {
                if matches!(value, Value::String(_)) {
                    return Ok(());
                }
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("expected String, got {type_name}"),
                ));
            }
            "Id" => match value {
                Value::String(s) if !s.is_empty() => return Ok(()),
                Value::String(_) => {
                    return Err(host.validation_error(
                        path,
                        "invalid_value",
                        "expected non-empty Id".to_string(),
                    ));
                }
                _ => {
                    return Err(host.validation_error(
                        path,
                        "type_mismatch",
                        format!("expected Id, got {type_name}"),
                    ));
                }
            },
            "Email" => match value {
                Value::String(s) if rt_validate::is_email(&s) => return Ok(()),
                Value::String(_) => {
                    return Err(host.validation_error(
                        path,
                        "invalid_value",
                        "invalid email address".to_string(),
                    ));
                }
                _ => {
                    return Err(host.validation_error(
                        path,
                        "type_mismatch",
                        format!("expected Email, got {type_name}"),
                    ));
                }
            },
            "Bytes" => {
                if matches!(value, Value::Bytes(_)) {
                    return Ok(());
                }
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("expected Bytes, got {type_name}"),
                ));
            }
            "Html" => {
                if matches!(value, Value::Html(_)) {
                    return Ok(());
                }
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    format!("expected Html, got {type_name}"),
                ));
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
        _ => Err(host.validation_error(
            path,
            "type_mismatch",
            format!("expected {name}, got {type_name}"),
        )),
    }
}

fn decode_simple_json<H: RuntimeTypeHost>(
    host: &H,
    json: &rt_json::JsonValue,
    name: &str,
    path: &str,
) -> Result<Option<Value>, H::Error> {
    let value = match name {
        "Int" => match json {
            rt_json::JsonValue::Number(n) if n.fract() == 0.0 => Value::Int(*n as i64),
            _ => {
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    "expected Int".to_string(),
                ));
            }
        },
        "Float" => match json {
            rt_json::JsonValue::Number(n) => Value::Float(*n),
            _ => {
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    "expected Float".to_string(),
                ));
            }
        },
        "Bool" => match json {
            rt_json::JsonValue::Bool(v) => Value::Bool(*v),
            _ => {
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    "expected Bool".to_string(),
                ));
            }
        },
        "String" | "Id" | "Email" => match json {
            rt_json::JsonValue::String(v) => Value::String(v.clone()),
            _ => {
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    "expected String".to_string(),
                ));
            }
        },
        "Bytes" => match json {
            rt_json::JsonValue::String(v) => {
                let bytes = rt_bytes::decode_base64(v).map_err(|msg| {
                    host.validation_error(
                        path,
                        "invalid_value",
                        format!("invalid Bytes (base64): {msg}"),
                    )
                })?;
                Value::Bytes(bytes)
            }
            _ => {
                return Err(host.validation_error(
                    path,
                    "type_mismatch",
                    "expected String".to_string(),
                ));
            }
        },
        "Html" => {
            return Err(host.validation_error(path, "type_mismatch", "expected Html".to_string()));
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn decode_json_result_value<H: RuntimeTypeHost>(
    host: &mut H,
    json: &rt_json::JsonValue,
    ok_ty: &TypeRef,
    err_ty: Option<&TypeRef>,
    path: &str,
) -> Result<Value, H::Error> {
    let rt_json::JsonValue::Object(map) = json else {
        return Err(host.validation_error(
            path,
            "type_mismatch",
            "expected Result object".to_string(),
        ));
    };
    let tag = map.get("type").ok_or_else(|| {
        host.validation_error(path, "missing_field", "missing Result tag".to_string())
    })?;
    let rt_json::JsonValue::String(tag) = tag else {
        return Err(host.validation_error(
            &format!("{path}.type"),
            "type_mismatch",
            "expected Result tag string".to_string(),
        ));
    };
    let data = map.get("data").ok_or_else(|| {
        host.validation_error(path, "missing_field", "missing Result data".to_string())
    })?;
    match tag.as_str() {
        "Ok" => {
            let value = decode_json_value(host, data, ok_ty, &format!("{path}.data"))?;
            Ok(Value::ResultOk(Box::new(value)))
        }
        "Err" => {
            let value = if let Some(err_ty) = err_ty {
                decode_json_value(host, data, err_ty, &format!("{path}.data"))?
            } else {
                json_to_value(data)
            };
            Ok(Value::ResultErr(Box::new(value)))
        }
        _ => Err(host.validation_error(
            &format!("{path}.type"),
            "invalid_value",
            format!("unknown Result variant {tag}"),
        )),
    }
}

pub fn value_to_json(value: &Value) -> rt_json::JsonValue {
    match value.unboxed() {
        Value::Unit => rt_json::JsonValue::Null,
        Value::Int(v) => rt_json::JsonValue::Number(v as f64),
        Value::Float(v) => rt_json::JsonValue::Number(v),
        Value::Bool(v) => rt_json::JsonValue::Bool(v),
        Value::String(v) => rt_json::JsonValue::String(v.clone()),
        Value::Bytes(v) => rt_json::JsonValue::String(rt_bytes::encode_base64(&v)),
        Value::Html(node) => rt_json::JsonValue::String(node.render_to_string()),
        Value::Null => rt_json::JsonValue::Null,
        Value::List(items) => rt_json::JsonValue::Array(items.iter().map(value_to_json).collect()),
        Value::Map(items) => {
            let mut out = BTreeMap::new();
            for (key, value) in items {
                out.insert(key.clone(), value_to_json(&value));
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
                out.insert(key.clone(), value_to_json(&value));
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
                    out.insert("data".to_string(), value_to_json(&payload[0]));
                }
                _ => {
                    let items = payload.iter().map(value_to_json).collect();
                    out.insert("data".to_string(), rt_json::JsonValue::Array(items));
                }
            }
            rt_json::JsonValue::Object(out)
        }
        Value::ResultOk(value) => value_to_json(value.as_ref()),
        Value::ResultErr(value) => value_to_json(value.as_ref()),
        Value::Config(name) => rt_json::JsonValue::String(name.clone()),
        Value::Function(func) => {
            rt_json::JsonValue::String(format!("{}::{}", func.module_id, func.name))
        }
        Value::Builtin(name) => rt_json::JsonValue::String(name.clone()),
        Value::EnumCtor { name, variant } => {
            rt_json::JsonValue::String(format!("{name}.{variant}"))
        }
    }
}

pub fn json_to_value(json: &rt_json::JsonValue) -> Value {
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
        rt_json::JsonValue::Array(items) => Value::List(items.iter().map(json_to_value).collect()),
        rt_json::JsonValue::Object(items) => {
            let mut out = HashMap::new();
            for (key, value) in items {
                out.insert(key.clone(), json_to_value(value));
            }
            Value::Map(out)
        }
    }
}

pub fn validation_field_value(path: &str, code: &str, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::String(path.to_string()));
    fields.insert("code".to_string(), Value::String(code.to_string()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Struct {
        name: "ValidationField".to_string(),
        fields,
    }
}

pub fn validation_error_value(path: &str, code: &str, message: impl Into<String>) -> Value {
    let field = validation_field_value(path, code, message);
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
