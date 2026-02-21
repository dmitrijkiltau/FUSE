use std::cell::RefCell;
use std::collections::HashMap;

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, Params, params_from_iter};

use crate::interp::Value;

pub struct Db {
    state: RefCell<DbState>,
}

pub const DEFAULT_DB_POOL_SIZE: usize = 1;

struct DbState {
    conns: Vec<Connection>,
    next_conn_idx: usize,
    tx_conn_idx: Option<usize>,
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
        Self::open_with_pool(url, DEFAULT_DB_POOL_SIZE)
    }

    pub fn open_with_pool(url: &str, pool_size: usize) -> Result<Self, String> {
        if pool_size < 1 {
            return Err("db pool size must be >= 1".to_string());
        }
        let path = parse_sqlite_url(url)?;
        let mut conns = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            let conn = Connection::open(path).map_err(|err| format!("db open failed: {err}"))?;
            conns.push(conn);
        }
        Ok(Self {
            state: RefCell::new(DbState {
                conns,
                next_conn_idx: 0,
                tx_conn_idx: None,
            }),
        })
    }

    pub fn exec(&self, sql: &str) -> Result<(), String> {
        self.exec_params(sql, &[])
    }

    pub fn exec_params(&self, sql: &str, params: &[Value]) -> Result<(), String> {
        let sql_params = params_to_sql(params)?;
        self.with_connection(|conn| {
            conn.execute(sql, params_from_iter(sql_params))
                .map(|_| ())
                .map_err(|err| format!("db exec failed: {err}"))
        })
    }

    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<usize, String> {
        self.with_connection(|conn| {
            conn.execute(sql, params)
                .map_err(|err| format!("db exec failed: {err}"))
        })
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
        self.with_connection(|conn| {
            let mut stmt = conn
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
            while let Some(row) = rows
                .next()
                .map_err(|err| format!("db query failed: {err}"))?
            {
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
        })
    }

    pub fn begin_transaction(&self) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        if state.tx_conn_idx.is_some() {
            return Err("db transaction already active".to_string());
        }
        let idx = state.take_connection_index();
        state.conns[idx]
            .execute("BEGIN", ())
            .map_err(|err| format!("db exec failed: {err}"))?;
        state.tx_conn_idx = Some(idx);
        Ok(())
    }

    pub fn commit_transaction(&self) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        let idx = state
            .tx_conn_idx
            .ok_or_else(|| "db transaction not active".to_string())?;
        state.conns[idx]
            .execute("COMMIT", ())
            .map_err(|err| format!("db exec failed: {err}"))?;
        state.tx_conn_idx = None;
        Ok(())
    }

    pub fn rollback_transaction(&self) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        let Some(idx) = state.tx_conn_idx else {
            return Ok(());
        };
        let result = state.conns[idx]
            .execute("ROLLBACK", ())
            .map(|_| ())
            .map_err(|err| format!("db exec failed: {err}"));
        state.tx_conn_idx = None;
        result
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.state.borrow_mut();
        let idx = state.connection_index();
        let conn = &state.conns[idx];
        f(conn)
    }
}

impl DbState {
    fn connection_index(&mut self) -> usize {
        if let Some(idx) = self.tx_conn_idx {
            return idx;
        }
        self.take_connection_index()
    }

    fn take_connection_index(&mut self) -> usize {
        let idx = self.next_conn_idx % self.conns.len();
        self.next_conn_idx = (idx + 1) % self.conns.len();
        idx
    }
}

fn invalid_pool_size_message(source: &str) -> String {
    format!("invalid {source}: expected integer >= 1")
}

pub fn parse_db_pool_size(raw: &str, source: &str) -> Result<usize, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_pool_size_message(source));
    }
    let value: i64 = trimmed
        .parse()
        .map_err(|_| invalid_pool_size_message(source))?;
    if value < 1 {
        return Err(invalid_pool_size_message(source));
    }
    usize::try_from(value).map_err(|_| invalid_pool_size_message(source))
}

pub fn parse_db_pool_size_value(value: &Value, source: &str) -> Result<usize, String> {
    match value.unboxed() {
        Value::Int(v) => {
            if v < 1 {
                return Err(invalid_pool_size_message(source));
            }
            usize::try_from(v).map_err(|_| invalid_pool_size_message(source))
        }
        Value::String(raw) => parse_db_pool_size(&raw, source),
        _ => Err(invalid_pool_size_message(source)),
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
        Value::Boxed(inner) => param_to_sql(&inner.lock().expect("box lock")),
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
        Value::Boxed(inner) => validate_param_value(&inner.lock().expect("box lock")),
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
        ValueRef::Text(bytes) => Value::String(String::from_utf8_lossy(bytes).to_string()),
        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_url(name: &str) -> String {
        let mut path = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!("{name}_{stamp}.sqlite"));
        format!("sqlite://{}", path.display())
    }

    fn scalar_i64(rows: &[HashMap<String, Value>], key: &str) -> i64 {
        let value = rows.first().and_then(|row| row.get(key));
        match value {
            Some(Value::Int(v)) => *v,
            _ => panic!("expected Int scalar for key {key}, got {value:?}"),
        }
    }

    #[test]
    fn parse_db_pool_size_accepts_positive_integers() {
        assert_eq!(parse_db_pool_size("1", "FUSE_DB_POOL_SIZE").unwrap(), 1);
        assert_eq!(parse_db_pool_size("8", "FUSE_DB_POOL_SIZE").unwrap(), 8);
        assert_eq!(parse_db_pool_size("  3  ", "FUSE_DB_POOL_SIZE").unwrap(), 3);
    }

    #[test]
    fn parse_db_pool_size_rejects_invalid_values() {
        for raw in ["", "0", "-1", "abc", "1.5"] {
            let err = parse_db_pool_size(raw, "FUSE_DB_POOL_SIZE").expect_err("expected failure");
            assert!(
                err.contains("FUSE_DB_POOL_SIZE"),
                "error should mention source, got: {err}"
            );
        }
    }

    #[test]
    fn parse_db_pool_size_value_supports_int_and_string() {
        assert_eq!(
            parse_db_pool_size_value(&Value::Int(2), "App.dbPoolSize").unwrap(),
            2
        );
        assert_eq!(
            parse_db_pool_size_value(&Value::String("4".to_string()), "App.dbPoolSize").unwrap(),
            4
        );
        let err = parse_db_pool_size_value(&Value::Int(0), "App.dbPoolSize").unwrap_err();
        assert!(
            err.contains("App.dbPoolSize"),
            "error should mention source, got: {err}"
        );
    }

    #[test]
    fn open_with_pool_uses_requested_size() {
        let db = Db::open_with_pool(&temp_db_url("fuse_db_pool_size"), 3).unwrap();
        let state = db.state.borrow();
        assert_eq!(state.conns.len(), 3);
    }

    #[test]
    fn transaction_scope_pins_single_connection() {
        let db = Db::open_with_pool(&temp_db_url("fuse_db_tx_scope"), 2).unwrap();
        db.exec("create table if not exists items (id integer)")
            .unwrap();

        db.begin_transaction().unwrap();
        db.exec("insert into items (id) values (1)").unwrap();
        let in_tx = db.query("select count(*) as c from items").unwrap();
        assert_eq!(scalar_i64(&in_tx, "c"), 1);
        db.rollback_transaction().unwrap();

        let after_rollback = db.query("select count(*) as c from items").unwrap();
        assert_eq!(scalar_i64(&after_rollback, "c"), 0);
    }
}
