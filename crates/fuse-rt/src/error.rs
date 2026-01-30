use std::collections::BTreeMap;

use crate::json::JsonValue;

#[derive(Clone, Debug)]
pub struct ValidationField {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ValidationError {
    pub message: String,
    pub fields: Vec<ValidationField>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fields: Vec::new(),
        }
    }

    pub fn to_json(&self) -> JsonValue {
        validation_error_json(&self.message, &self.fields)
    }
}

pub fn validation_error_json(message: &str, fields: &[ValidationField]) -> JsonValue {
    error_json("validation_error", message, Some(fields))
}

pub fn error_json(code: &str, message: &str, fields: Option<&[ValidationField]>) -> JsonValue {
    let mut err = BTreeMap::new();
    err.insert("code".to_string(), JsonValue::String(code.to_string()));
    err.insert(
        "message".to_string(),
        JsonValue::String(message.to_string()),
    );
    if let Some(fields) = fields {
        let items = fields.iter().map(|field| field.to_json()).collect();
        err.insert("fields".to_string(), JsonValue::Array(items));
    }
    let mut root = BTreeMap::new();
    root.insert("error".to_string(), JsonValue::Object(err));
    JsonValue::Object(root)
}

impl ValidationField {
    pub fn to_json(&self) -> JsonValue {
        let mut map = BTreeMap::new();
        map.insert("path".to_string(), JsonValue::String(self.path.clone()));
        map.insert("code".to_string(), JsonValue::String(self.code.clone()));
        map.insert(
            "message".to_string(),
            JsonValue::String(self.message.clone()),
        );
        JsonValue::Object(map)
    }
}
