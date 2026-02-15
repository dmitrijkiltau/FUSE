use std::collections::BTreeMap;

use crate::error::{ValidationError, ValidationField};
use crate::json::JsonValue;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Struct {
        name: String,
        fields: BTreeMap<String, Value>,
    },
    Enum {
        name: String,
        variant: String,
        payload: Vec<Value>,
    },
}

#[derive(Clone, Debug)]
pub enum Type {
    Null,
    Bool,
    Int,
    Float,
    String,
    Option(Box<Type>),
    List(Box<Type>),
    Map(Box<Type>),
    Struct(StructType),
    Enum(EnumType),
}

#[derive(Clone, Debug)]
pub struct StructField {
    pub name: String,
    pub ty: Type,
    pub default: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct StructType {
    pub name: String,
    pub fields: Vec<StructField>,
}

#[derive(Clone, Debug)]
pub struct EnumVariant {
    pub name: String,
    pub payload: Vec<Type>,
}

#[derive(Clone, Debug)]
pub struct EnumType {
    pub name: String,
    pub variants: Vec<EnumVariant>,
}

pub fn encode_value(value: &Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Null,
        Value::Bool(v) => JsonValue::Bool(*v),
        Value::Int(v) => JsonValue::Number(*v as f64),
        Value::Float(v) => JsonValue::Number(*v),
        Value::String(v) => JsonValue::String(v.clone()),
        Value::List(items) => JsonValue::Array(items.iter().map(encode_value).collect()),
        Value::Map(map) => {
            let mut out = BTreeMap::new();
            for (key, value) in map {
                out.insert(key.clone(), encode_value(value));
            }
            JsonValue::Object(out)
        }
        Value::Struct { fields, .. } => {
            let mut out = BTreeMap::new();
            for (key, value) in fields {
                out.insert(key.clone(), encode_value(value));
            }
            JsonValue::Object(out)
        }
        Value::Enum {
            variant, payload, ..
        } => {
            let mut out = BTreeMap::new();
            out.insert("type".to_string(), JsonValue::String(variant.clone()));
            match payload.len() {
                0 => {}
                1 => {
                    out.insert("data".to_string(), encode_value(&payload[0]));
                }
                _ => {
                    out.insert(
                        "data".to_string(),
                        JsonValue::Array(payload.iter().map(encode_value).collect()),
                    );
                }
            }
            JsonValue::Object(out)
        }
    }
}

pub fn decode_value(value: &JsonValue, ty: &Type) -> Result<Value, ValidationError> {
    let mut ctx = DecodeCtx::new();
    let decoded = ctx.decode(value, ty, &Path::root());
    if ctx.fields.is_empty() {
        decoded.ok_or_else(|| ValidationError::new("validation failed"))
    } else {
        Err(ValidationError {
            message: "validation failed".to_string(),
            fields: ctx.fields,
        })
    }
}

struct DecodeCtx {
    fields: Vec<ValidationField>,
}

impl DecodeCtx {
    fn new() -> Self {
        Self { fields: Vec::new() }
    }

    fn decode(&mut self, value: &JsonValue, ty: &Type, path: &Path) -> Option<Value> {
        match ty {
            Type::Null => match value {
                JsonValue::Null => Some(Value::Null),
                _ => {
                    self.push_error(path, "invalid_type", "expected null");
                    None
                }
            },
            Type::Bool => match value {
                JsonValue::Bool(v) => Some(Value::Bool(*v)),
                _ => {
                    self.push_error(path, "invalid_type", "expected bool");
                    None
                }
            },
            Type::Int => match value {
                JsonValue::Number(v) if v.fract() == 0.0 => Some(Value::Int(*v as i64)),
                JsonValue::Number(_) => {
                    self.push_error(path, "invalid_type", "expected int");
                    None
                }
                _ => {
                    self.push_error(path, "invalid_type", "expected int");
                    None
                }
            },
            Type::Float => match value {
                JsonValue::Number(v) => Some(Value::Float(*v)),
                _ => {
                    self.push_error(path, "invalid_type", "expected float");
                    None
                }
            },
            Type::String => match value {
                JsonValue::String(v) => Some(Value::String(v.clone())),
                _ => {
                    self.push_error(path, "invalid_type", "expected string");
                    None
                }
            },
            Type::Option(inner) => match value {
                JsonValue::Null => Some(Value::Null),
                other => self.decode(other, inner, path),
            },
            Type::List(inner) => match value {
                JsonValue::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for (idx, item) in items.iter().enumerate() {
                        let child_path = path.index(idx);
                        if let Some(val) = self.decode(item, inner, &child_path) {
                            out.push(val);
                        }
                    }
                    Some(Value::List(out))
                }
                _ => {
                    self.push_error(path, "invalid_type", "expected array");
                    None
                }
            },
            Type::Map(inner) => match value {
                JsonValue::Object(map) => {
                    let mut out = BTreeMap::new();
                    for (key, val) in map {
                        let child_path = path.field(key);
                        if let Some(decoded) = self.decode(val, inner, &child_path) {
                            out.insert(key.clone(), decoded);
                        }
                    }
                    Some(Value::Map(out))
                }
                _ => {
                    self.push_error(path, "invalid_type", "expected object");
                    None
                }
            },
            Type::Struct(ty) => self.decode_struct(value, ty, path),
            Type::Enum(ty) => self.decode_enum(value, ty, path),
        }
    }

    fn decode_struct(&mut self, value: &JsonValue, ty: &StructType, path: &Path) -> Option<Value> {
        let obj = match value {
            JsonValue::Object(map) => map,
            _ => {
                self.push_error(path, "invalid_type", "expected object");
                return None;
            }
        };

        let mut fields = BTreeMap::new();
        let mut known = BTreeMap::new();
        for field in &ty.fields {
            known.insert(field.name.as_str(), field);
        }

        for key in obj.keys() {
            if !known.contains_key(key.as_str()) {
                let child_path = path.field(key);
                self.push_error(&child_path, "unknown_field", "unknown field");
            }
        }

        for field in &ty.fields {
            let child_path = path.field(&field.name);
            match obj.get(&field.name) {
                Some(val) => {
                    if let Some(decoded) = self.decode(val, &field.ty, &child_path) {
                        fields.insert(field.name.clone(), decoded);
                    }
                }
                None => {
                    if let Some(default) = &field.default {
                        fields.insert(field.name.clone(), default.clone());
                    } else if is_optional(&field.ty) {
                        fields.insert(field.name.clone(), Value::Null);
                    } else {
                        self.push_error(&child_path, "missing_field", "missing field");
                    }
                }
            }
        }

        Some(Value::Struct {
            name: ty.name.clone(),
            fields,
        })
    }

    fn decode_enum(&mut self, value: &JsonValue, ty: &EnumType, path: &Path) -> Option<Value> {
        let obj = match value {
            JsonValue::Object(map) => map,
            _ => {
                self.push_error(path, "invalid_type", "expected object");
                return None;
            }
        };
        let kind = match obj.get("type") {
            Some(JsonValue::String(v)) => v.clone(),
            _ => {
                let child_path = path.field("type");
                self.push_error(&child_path, "missing_field", "expected enum tag");
                return None;
            }
        };
        let variant = match ty.variants.iter().find(|v| v.name == kind) {
            Some(variant) => variant,
            None => {
                let child_path = path.field("type");
                self.push_error(&child_path, "invalid_variant", "unknown variant");
                return None;
            }
        };
        let payload = match variant.payload.len() {
            0 => Vec::new(),
            1 => {
                let data = match obj.get("data") {
                    Some(val) => val,
                    None => {
                        let child_path = path.field("data");
                        self.push_error(&child_path, "missing_field", "missing payload");
                        return None;
                    }
                };
                let child_path = path.field("data");
                match self.decode(data, &variant.payload[0], &child_path) {
                    Some(decoded) => vec![decoded],
                    None => Vec::new(),
                }
            }
            _ => {
                let data = match obj.get("data") {
                    Some(val) => val,
                    None => {
                        let child_path = path.field("data");
                        self.push_error(&child_path, "missing_field", "missing payload");
                        return None;
                    }
                };
                let array = match data {
                    JsonValue::Array(items) => items,
                    _ => {
                        let child_path = path.field("data");
                        self.push_error(&child_path, "invalid_type", "expected array payload");
                        return None;
                    }
                };
                if array.len() != variant.payload.len() {
                    let child_path = path.field("data");
                    self.push_error(&child_path, "invalid_length", "payload length mismatch");
                    return None;
                }
                let mut decoded = Vec::new();
                for (idx, (item, ty)) in array.iter().zip(variant.payload.iter()).enumerate() {
                    let child_path = path.field("data").index(idx);
                    if let Some(val) = self.decode(item, ty, &child_path) {
                        decoded.push(val);
                    }
                }
                decoded
            }
        };
        Some(Value::Enum {
            name: ty.name.clone(),
            variant: variant.name.clone(),
            payload,
        })
    }

    fn push_error(&mut self, path: &Path, code: &str, message: &str) {
        self.fields.push(ValidationField {
            path: path.to_string(),
            code: code.to_string(),
            message: message.to_string(),
        });
    }
}

fn is_optional(ty: &Type) -> bool {
    matches!(ty, Type::Option(_))
}

#[derive(Clone, Debug)]
struct Path {
    segments: Vec<Segment>,
}

#[derive(Clone, Debug)]
enum Segment {
    Field(String),
    Index(usize),
}

impl Path {
    fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    fn field(&self, name: &str) -> Self {
        let mut segments = self.segments.clone();
        segments.push(Segment::Field(name.to_string()));
        Self { segments }
    }

    fn index(&self, idx: usize) -> Self {
        let mut segments = self.segments.clone();
        segments.push(Segment::Index(idx));
        Self { segments }
    }

    fn to_string(&self) -> String {
        if self.segments.is_empty() {
            return "$".to_string();
        }
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                Segment::Field(name) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(name);
                }
                Segment::Index(idx) => {
                    out.push('[');
                    out.push_str(&idx.to_string());
                    out.push(']');
                }
            }
        }
        out
    }
}
