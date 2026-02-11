use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::db::Db;
use fusec::interp::Value;

fn write_temp_file(name: &str, ext: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    path.push(format!("{name}_{stamp}.{ext}"));
    fs::write(&path, contents).expect("failed to write temp file");
    path
}

fn run_program_with_args(backend: &str, source: &str, args: &[&str]) -> std::process::Output {
    let program_path = write_temp_file("fuse_bytes_runtime", "fuse", source);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&program_path)
        .arg("--");
    for arg in args {
        cmd.arg(arg);
    }
    cmd.output().expect("failed to run fusec")
}

#[test]
fn bytes_cli_roundtrip_across_backends() {
    let program = r#"
fn main(token: Bytes):
  print(token)
"#;

    for backend in ["ast", "vm", "native"] {
        let output = run_program_with_args(backend, program, &["--token=YQ=="]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "YQ==", "{backend} stdout");
    }
}

#[test]
fn bytes_json_roundtrip_ast_vm() {
    let program = r#"
fn main(token: Bytes):
  print(json.encode({"blob": token}))
"#;

    for backend in ["ast", "vm"] {
        let output = run_program_with_args(backend, program, &["--token=YQ=="]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), r#"{"blob":"YQ=="}"#, "{backend} stdout");
    }
}

#[test]
fn bytes_cli_rejects_invalid_base64() {
    let program = r#"
fn main(token: Bytes):
  print(token)
"#;

    for backend in ["ast", "vm", "native"] {
        let output = run_program_with_args(backend, program, &["--token=not_base64"]);
        assert!(
            !output.status.success(),
            "{backend} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("invalid Bytes (base64)"),
            "{backend} stderr: {stderr}"
        );
    }
}

#[test]
fn db_blob_roundtrip_uses_bytes_value() {
    let mut db_path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    db_path.push(format!("fuse_bytes_db_{stamp}.sqlite"));
    let db_url = format!("sqlite://{}", db_path.display());
    let db = Db::open(&db_url).expect("db open");

    db.exec("create table bytes_test (id text primary key, blob blob, note text)")
        .expect("create table");
    db.exec_params(
        "insert into bytes_test (id, blob, note) values (?, ?, ?)",
        &[
            Value::String("row-1".to_string()),
            Value::Bytes(vec![0, 1, 2, 255]),
            Value::String("ok".to_string()),
        ],
    )
    .expect("insert");

    let rows = db
        .query_params(
            "select id, blob, note from bytes_test where id = ?",
            &[Value::String("row-1".to_string())],
        )
        .expect("query");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    match row.get("id") {
        Some(Value::String(v)) => assert_eq!(v, "row-1"),
        other => panic!("unexpected id value: {other:?}"),
    }
    match row.get("note") {
        Some(Value::String(v)) => assert_eq!(v, "ok"),
        other => panic!("unexpected note value: {other:?}"),
    }
    match row.get("blob") {
        Some(Value::Bytes(v)) => assert_eq!(v.as_slice(), &[0, 1, 2, 255]),
        other => panic!("unexpected blob value: {other:?}"),
    }
}
