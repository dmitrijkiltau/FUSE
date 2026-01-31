use std::collections::HashMap;

use rusqlite::{types::ValueRef, Connection, Params};

use crate::interp::Value;

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(url: &str) -> Result<Self, String> {
        let path = parse_sqlite_url(url)?;
        let conn = Connection::open(path).map_err(|err| format!("db open failed: {err}"))?;
        Ok(Self { conn })
    }

    pub fn exec(&self, sql: &str) -> Result<(), String> {
        self.conn
            .execute_batch(sql)
            .map_err(|err| format!("db exec failed: {err}"))
    }

    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<usize, String> {
        self.conn
            .execute(sql, params)
            .map_err(|err| format!("db exec failed: {err}"))
    }

    pub fn query(&self, sql: &str) -> Result<Vec<HashMap<String, Value>>, String> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|err| format!("db query failed: {err}"))?;
        let columns: Vec<String> = stmt
            .column_names()
            .iter()
            .map(|name| name.to_string())
            .collect();
        let mut rows = stmt
            .query([])
            .map_err(|err| format!("db query failed: {err}"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|err| format!("db query failed: {err}"))? {
            let mut map = HashMap::new();
            for (idx, name) in columns.iter().enumerate() {
                let value = row
                    .get_ref(idx)
                    .map_err(|err| format!("db query failed: {err}"))?;
                map.insert(name.clone(), value_from_ref(value));
            }
            out.push(map);
        }
        Ok(out)
    }
}

fn parse_sqlite_url(url: &str) -> Result<&str, String> {
    let url = url.trim();
    if let Some(path) = url.strip_prefix("sqlite://") {
        if path.is_empty() {
            return Err("sqlite url missing path".to_string());
        }
        return Ok(path);
    }
    if let Some(path) = url.strip_prefix("sqlite:") {
        if path.is_empty() {
            return Err("sqlite url missing path".to_string());
        }
        return Ok(path);
    }
    Err("unsupported db url (expected sqlite://...)".to_string())
}

fn value_from_ref(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => Value::Int(v),
        ValueRef::Real(v) => Value::Float(v),
        ValueRef::Text(bytes) => {
            Value::String(String::from_utf8_lossy(bytes).to_string())
        }
        ValueRef::Blob(bytes) => Value::String(bytes_to_hex(bytes)),
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}
