use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::db::Db;
use fusec::interp::Value;

fn write_temp_program(name: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("{name}_{stamp}.fuse"));
    fs::write(&path, contents).expect("failed to write temp program");
    path
}

fn temp_db_url() -> String {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("fuse_db_upsert_runtime_{stamp}.sqlite"));
    format!("sqlite://{}", path.display())
}

fn run_program(backend: &str, path: &PathBuf, db_url: &str) -> Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(path)
        .env("FUSE_DB_URL", db_url)
        .output()
        .expect("failed to run fusec --run")
}

fn scalar_i64(rows: &[HashMap<String, Value>], key: &str) -> i64 {
    let value = rows.first().and_then(|row| row.get(key));
    match value {
        Some(Value::Int(v)) => *v,
        _ => panic!("expected Int scalar for key {key}, got {value:?}"),
    }
}

fn scalar_string(rows: &[HashMap<String, Value>], key: &str) -> String {
    let value = rows.first().and_then(|row| row.get(key));
    match value {
        Some(Value::String(v)) => v.clone(),
        _ => panic!("expected String scalar for key {key}, got {value:?}"),
    }
}

#[test]
fn query_upsert_has_ast_native_parity() {
    let program = r#"
requires db

type User:
  id: Int
  name: String

fn main():
  db.exec("create table if not exists users (id integer primary key, name text not null)")
  db.exec("delete from users")
  db.from("users").upsert(User(id=1, name="Ada")).exec()
  db.from("users").upsert(User(id=1, name="Ava")).exec()

app "demo":
  main()
"#;
    let path = write_temp_program("fuse_db_upsert_runtime", program);

    for backend in ["ast", "native"] {
        let db_url = temp_db_url();
        let output = run_program(backend, &path, &db_url);
        assert!(
            output.status.success(),
            "backend={backend} expected success, stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let verify_db = Db::open_with_pool(&db_url, 1).expect("open verify db");
        let rows = verify_db
            .query("select count(*) as c from users")
            .expect("count users");
        assert_eq!(scalar_i64(&rows, "c"), 1, "backend={backend}");

        let row = verify_db
            .query("select name from users where id = 1")
            .expect("query user");
        assert_eq!(scalar_string(&row, "name"), "Ava", "backend={backend}");
    }
}
