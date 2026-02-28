use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::db::Db;
use fusec::interp::Interpreter;
use fusec::native::{NativeVm, compile_registry};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn temp_project_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("fuse_transaction_runtime_{tag}_{nanos}"));
    dir
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, contents).expect("write source file");
}

fn temp_db_url(tag: &str) -> String {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("fuse_transaction_runtime_{tag}_{nanos}.sqlite"));
    format!("sqlite://{}", path.display())
}

fn row_count(db_url: &str) -> i64 {
    let db = Db::open_with_pool(db_url, 1).expect("open db");
    let rows = db
        .query("select count(*) as c from items")
        .expect("query row count");
    let value = rows.first().and_then(|row| row.get("c"));
    match value {
        Some(fusec::interp::Value::Int(v)) => *v,
        other => panic!("expected Int count row, got {other:?}"),
    }
}

fn program_source() -> &'static str {
    r#"
requires db

fn reset():
  db.exec("create table if not exists items (id integer)")
  db.exec("delete from items")

fn tx_commit():
  transaction:
    db.exec("insert into items (id) values (1)")

fn tx_rollback():
  transaction:
    db.exec("insert into items (id) values (2)")
    assert(false, "boom")
"#
}

#[test]
fn transaction_commits_and_rolls_back_in_ast_backend() {
    let _env_guard = ENV_LOCK.lock().expect("lock env guard");
    let dir = temp_project_dir("ast");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(&main_path, program_source());
    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let db_url = temp_db_url("ast");
    unsafe {
        std::env::set_var("FUSE_DB_URL", &db_url);
    }
    let mut interp = Interpreter::with_registry(&registry);
    interp
        .call_function_with_named_args("reset", &HashMap::new())
        .expect("reset failed");
    interp
        .call_function_with_named_args("tx_commit", &HashMap::new())
        .expect("tx_commit failed");
    let err = interp.call_function_with_named_args("tx_rollback", &HashMap::new());
    assert!(err.is_err(), "expected tx_rollback to fail");
    let err = err.err().unwrap_or_default();
    assert!(err.contains("boom"), "unexpected tx_rollback error: {err}");

    assert_eq!(row_count(&db_url), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn transaction_commits_and_rolls_back_in_native_backend() {
    let _env_guard = ENV_LOCK.lock().expect("lock env guard");
    let dir = temp_project_dir("native");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(&main_path, program_source());
    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let db_url = temp_db_url("native");
    unsafe {
        std::env::set_var("FUSE_DB_URL", &db_url);
    }
    let native = compile_registry(&registry).expect("native lowering failed");
    let mut vm = NativeVm::new(&native);
    vm.call_function("reset", vec![]).expect("reset failed");
    vm.call_function("tx_commit", vec![])
        .expect("tx_commit failed");
    let err = vm.call_function("tx_rollback", vec![]);
    assert!(err.is_err(), "expected tx_rollback to fail");
    let err = err.err().unwrap_or_default();
    assert!(err.contains("boom"), "unexpected tx_rollback error: {err}");

    assert_eq!(row_count(&db_url), 1);
    let _ = fs::remove_dir_all(&dir);
}
