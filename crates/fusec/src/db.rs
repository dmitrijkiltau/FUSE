use std::collections::HashMap;

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection, Params};

use crate::interp::Value;

pub struct Db {
    conn: Connection,
}

#[derive(Clone, Debug)]
pub struct Query {
    table: String,
    select: Vec<String>,
    wheres: Vec<WhereClause>,
    order_by: Option<(String, OrderDir)>,
    limit: Option<i64>,
}

#[derive(Clone, Debug)]
struct WhereClause {
    column: String,
    op: WhereOp,
    values: Vec<Value>,
}

#[derive(Copy, Clone, Debug)]
enum OrderDir {
    Asc,
    Desc,
}

#[derive(Copy, Clone, Debug)]
enum WhereOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    In,
}

impl Query {
    pub fn new(table: String) -> Result<Self, String> {
        if !is_valid_identifier(&table) {
            return Err("invalid table name".to_string());
        }
        Ok(Self {
            table,
            select: Vec::new(),
            wheres: Vec::new(),
            order_by: None,
            limit: None,
        })
    }

    pub fn select(&self, columns: Vec<String>) -> Result<Self, String> {
        if columns.is_empty() {
            return Err("select expects at least one column".to_string());
        }
        for col in &columns {
            if !is_valid_identifier(col) {
                return Err(format!("invalid column name {col}"));
            }
        }
        let mut next = self.clone();
        next.select = columns;
        Ok(next)
    }

    pub fn where_clause(&self, column: String, op: String, value: Value) -> Result<Self, String> {
        if !is_valid_identifier(&column) {
            return Err(format!("invalid column name {column}"));
        }
        let op = parse_where_op(&op)?;
        let values = if matches!(op, WhereOp::In) {
            match value.unboxed() {
                Value::List(items) => {
                    if items.is_empty() {
                        return Err("in expects a non-empty list".to_string());
                    }
                    for item in &items {
                        validate_param_value(item)?;
                    }
                    items
                }
                _ => return Err("in expects a list value".to_string()),
            }
        } else {
            validate_param_value(&value)?;
            vec![value]
        };
        let mut next = self.clone();
        next.wheres.push(WhereClause { column, op, values });
        Ok(next)
    }

    pub fn order_by(&self, column: String, dir: String) -> Result<Self, String> {
        if !is_valid_identifier(&column) {
            return Err(format!("invalid column name {column}"));
        }
        let dir = parse_order_dir(&dir)?;
        let mut next = self.clone();
        next.order_by = Some((column, dir));
        Ok(next)
    }

    pub fn limit(&self, value: i64) -> Result<Self, String> {
        if value < 0 {
            return Err("limit must be >= 0".to_string());
        }
        let mut next = self.clone();
        next.limit = Some(value);
        Ok(next)
    }

    pub fn sql(&self) -> Result<String, String> {
        self.build_sql(None).map(|(sql, _)| sql)
    }

    pub fn params(&self) -> Result<Vec<Value>, String> {
        self.build_sql(None).map(|(_, params)| params)
    }

    pub fn build_sql(&self, limit_override: Option<i64>) -> Result<(String, Vec<Value>), String> {
        let mut sql = String::new();
        if self.select.is_empty() {
            sql.push_str("select * from ");
        } else {
            sql.push_str("select ");
            sql.push_str(&self.select.join(", "));
            sql.push_str(" from ");
        }
        sql.push_str(&self.table);
        let mut params = Vec::new();
        if !self.wheres.is_empty() {
            sql.push_str(" where ");
            for (idx, clause) in self.wheres.iter().enumerate() {
                if idx > 0 {
                    sql.push_str(" and ");
                }
                sql.push_str(&clause.column);
                sql.push(' ');
                sql.push_str(clause.op.as_str());
                if matches!(clause.op, WhereOp::In) {
                    sql.push_str(" (");
                    let mut placeholders = Vec::with_capacity(clause.values.len());
                    for _ in &clause.values {
                        placeholders.push("?");
                    }
                    sql.push_str(&placeholders.join(", "));
                    sql.push(')');
                } else {
                    sql.push_str(" ?");
                }
                params.extend(clause.values.iter().cloned());
            }
        }
        if let Some((column, dir)) = &self.order_by {
            sql.push_str(" order by ");
            sql.push_str(column);
            sql.push(' ');
            sql.push_str(dir.as_str());
        }
        let limit = self.limit.or(limit_override);
        if let Some(limit) = limit {
            sql.push_str(" limit ?");
            params.push(Value::Int(limit));
        }
        Ok((sql, params))
    }
}

impl Db {
    pub fn open(url: &str) -> Result<Self, String> {
        let path = parse_sqlite_url(url)?;
        let conn = Connection::open(path).map_err(|err| format!("db open failed: {err}"))?;
        Ok(Self { conn })
    }

    pub fn exec(&self, sql: &str) -> Result<(), String> {
        self.exec_params(sql, &[])
    }

    pub fn exec_params(&self, sql: &str, params: &[Value]) -> Result<(), String> {
        let sql_params = params_to_sql(params)?;
        self.conn
            .execute(sql, params_from_iter(sql_params))
            .map(|_| ())
            .map_err(|err| format!("db exec failed: {err}"))
    }

    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<usize, String> {
        self.conn
            .execute(sql, params)
            .map_err(|err| format!("db exec failed: {err}"))
    }

    pub fn query(&self, sql: &str) -> Result<Vec<HashMap<String, Value>>, String> {
        self.query_params(sql, &[])
    }

    pub fn query_params(
        &self,
        sql: &str,
        params: &[Value],
    ) -> Result<Vec<HashMap<String, Value>>, String> {
        let sql_params = params_to_sql(params)?;
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
            .query(params_from_iter(sql_params))
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

fn params_to_sql(params: &[Value]) -> Result<Vec<SqlValue>, String> {
    let mut out = Vec::with_capacity(params.len());
    for param in params {
        out.push(param_to_sql(param)?);
    }
    Ok(out)
}

fn param_to_sql(param: &Value) -> Result<SqlValue, String> {
    match param.unboxed() {
        Value::Null => Ok(SqlValue::Null),
        Value::Int(v) => Ok(SqlValue::Integer(v)),
        Value::Float(v) => Ok(SqlValue::Real(v)),
        Value::Bool(v) => Ok(SqlValue::Integer(if v { 1 } else { 0 })),
        Value::String(v) => Ok(SqlValue::Text(v)),
        Value::Bytes(v) => Ok(SqlValue::Blob(v)),
        Value::Boxed(inner) => param_to_sql(&inner.borrow()),
        Value::ResultOk(inner) => param_to_sql(&inner),
        Value::ResultErr(inner) => param_to_sql(&inner),
        other => Err(format!(
            "unsupported db param type {}",
            other.to_string_value()
        )),
    }
}

fn validate_param_value(value: &Value) -> Result<(), String> {
    match value.unboxed() {
        Value::Null
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Bytes(_) => Ok(()),
        Value::Boxed(inner) => validate_param_value(&inner.borrow()),
        Value::ResultOk(inner) => validate_param_value(&inner),
        Value::ResultErr(inner) => validate_param_value(&inner),
        other => Err(format!(
            "unsupported db param type {}",
            other.to_string_value()
        )),
    }
}

fn parse_where_op(raw: &str) -> Result<WhereOp, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "=" => Ok(WhereOp::Eq),
        "!=" => Ok(WhereOp::Ne),
        "<" => Ok(WhereOp::Lt),
        "<=" => Ok(WhereOp::Le),
        ">" => Ok(WhereOp::Gt),
        ">=" => Ok(WhereOp::Ge),
        "like" => Ok(WhereOp::Like),
        "in" => Ok(WhereOp::In),
        _ => Err("unsupported where operator".to_string()),
    }
}

fn parse_order_dir(raw: &str) -> Result<OrderDir, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "asc" => Ok(OrderDir::Asc),
        "desc" => Ok(OrderDir::Desc),
        _ => Err("order_by expects asc or desc".to_string()),
    }
}

impl OrderDir {
    fn as_str(&self) -> &'static str {
        match self {
            OrderDir::Asc => "asc",
            OrderDir::Desc => "desc",
        }
    }
}

impl WhereOp {
    fn as_str(&self) -> &'static str {
        match self {
            WhereOp::Eq => "=",
            WhereOp::Ne => "!=",
            WhereOp::Lt => "<",
            WhereOp::Le => "<=",
            WhereOp::Gt => ">",
            WhereOp::Ge => ">=",
            WhereOp::Like => "like",
            WhereOp::In => "in",
        }
    }
}

fn is_valid_identifier(value: &str) -> bool {
    let mut parts = value.split('.');
    let first = match parts.next() {
        Some(part) => part,
        None => return false,
    };
    let second = parts.next();
    if parts.next().is_some() {
        return false;
    }
    if !is_valid_ident_part(first) {
        return false;
    }
    if let Some(part) = second {
        if !is_valid_ident_part(part) {
            return false;
        }
    }
    true
}

fn is_valid_ident_part(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '_') {
            return false;
        }
    }
    true
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
        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
    }
}
