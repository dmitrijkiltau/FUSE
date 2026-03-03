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

fn temp_workspace_dir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("{name}_{stamp}"));
    fs::create_dir_all(&path).expect("create workspace dir");
    path
}

fn temp_db_url() -> String {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("fuse_migration_pool_{stamp}.sqlite"));
    format!("sqlite://{}", path.display())
}

fn run_migrate(program_path: &PathBuf, db_url: &str, pool_size: usize) -> Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    Command::new(exe)
        .arg("--migrate")
        .arg(program_path)
        .env("FUSE_DB_URL", db_url)
        .env("FUSE_DB_POOL_SIZE", pool_size.to_string())
        .env_remove("APP_DB_POOL_SIZE")
        .output()
        .expect("failed to run fusec --migrate")
}

fn scalar_i64(rows: &[HashMap<String, Value>], key: &str) -> i64 {
    let value = rows.first().and_then(|row| row.get(key));
    match value {
        Some(Value::Int(v)) => *v,
        _ => panic!("expected Int scalar for key {key}, got {value:?}"),
    }
}

#[test]
fn migration_failure_rolls_back_data_and_history_with_pool() {
    let db_url = temp_db_url();
    let seed_db = Db::open_with_pool(&db_url, 1).expect("open db");
    seed_db
        .exec("create table items (id integer)")
        .expect("create table");

    let program = r#"
requires db

migration "001_fail_insert":
  db.exec("insert into items (id) values (1)")
  assert(false, "boom")
"#;
    let path = write_temp_program("fuse_migration_rollback", program);

    let output = run_migrate(&path, &db_url, 3);
    assert!(
        !output.status.success(),
        "expected migration failure, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let verify_db = Db::open_with_pool(&db_url, 1).expect("open verify db");
    let item_rows = verify_db
        .query("select count(*) as c from items")
        .expect("query items");
    assert_eq!(scalar_i64(&item_rows, "c"), 0);

    let migration_rows = verify_db
        .query(
            "select count(*) as c from __fuse_migrations where package = '' and name = '001_fail_insert'",
        )
        .expect("query migration history");
    assert_eq!(scalar_i64(&migration_rows, "c"), 0);
}

#[test]
fn migration_success_commits_data_and_history_with_pool() {
    let db_url = temp_db_url();
    let seed_db = Db::open_with_pool(&db_url, 1).expect("open db");
    seed_db
        .exec("create table items (id integer)")
        .expect("create table");

    let program = r#"
requires db

migration "001_insert":
  db.exec("insert into items (id) values (1)")
"#;
    let path = write_temp_program("fuse_migration_commit", program);

    let output = run_migrate(&path, &db_url, 3);
    assert!(
        output.status.success(),
        "expected migration success, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let verify_db = Db::open_with_pool(&db_url, 1).expect("open verify db");
    let item_rows = verify_db
        .query("select count(*) as c from items")
        .expect("query items");
    assert_eq!(scalar_i64(&item_rows, "c"), 1);

    let migration_rows = verify_db
        .query(
            "select count(*) as c from __fuse_migrations where package = '' and name = '001_insert'",
        )
        .expect("query migration history");
    assert_eq!(scalar_i64(&migration_rows, "c"), 1);
}

#[test]
fn migration_legacy_history_is_upgraded_without_reapplying() {
    let db_url = temp_db_url();
    let seed_db = Db::open_with_pool(&db_url, 1).expect("open db");
    seed_db
        .exec("create table items (id integer)")
        .expect("create items table");
    seed_db
        .exec("create table __fuse_migrations (id text primary key, applied_at text not null)")
        .expect("create legacy migration table");
    seed_db
        .exec_params(
            "insert into __fuse_migrations (id, applied_at) values (?, CURRENT_TIMESTAMP)",
            &[Value::String("001_insert".to_string())],
        )
        .expect("seed legacy migration row");

    let program = r#"
requires db

migration "001_insert":
  db.exec("insert into items (id) values (1)")
"#;
    let path = write_temp_program("fuse_migration_legacy_upgrade", program);

    let output = run_migrate(&path, &db_url, 2);
    assert!(
        output.status.success(),
        "expected migration success, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let verify_db = Db::open_with_pool(&db_url, 1).expect("open verify db");
    let item_rows = verify_db
        .query("select count(*) as c from items")
        .expect("query items");
    assert_eq!(scalar_i64(&item_rows, "c"), 0);

    let migration_rows = verify_db
        .query(
            "select count(*) as c from __fuse_migrations where package = '' and name = '001_insert'",
        )
        .expect("query migration history");
    assert_eq!(scalar_i64(&migration_rows, "c"), 1);

    let schema_rows = verify_db
        .query("pragma table_info(__fuse_migrations)")
        .expect("query migration schema");
    let columns: Vec<String> = schema_rows
        .into_iter()
        .filter_map(|row| match row.get("name") {
            Some(Value::String(name)) => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert!(columns.contains(&"package".to_string()));
    assert!(columns.contains(&"name".to_string()));
    assert!(!columns.contains(&"id".to_string()));
}

#[test]
fn migration_namespaces_allow_same_name_across_packages() {
    let db_url = temp_db_url();
    let seed_db = Db::open_with_pool(&db_url, 1).expect("open db");
    seed_db
        .exec("create table items (id integer primary key, source text not null)")
        .expect("create table");

    let dir = temp_workspace_dir("fuse_migration_namespace");
    let dep_dir = dir.join("deps").join("auth");
    fs::create_dir_all(&dep_dir).expect("create dep dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"[package]
entry = "main.fuse"
"#,
    )
    .expect("write root manifest");
    fs::write(
        dep_dir.join("fuse.toml"),
        r#"[package]
name = "auth"
entry = "main.fuse"
"#,
    )
    .expect("write dep manifest");
    fs::write(
        dir.join("main.fuse"),
        r#"requires db
import auth from "./deps/auth/main"

migration "001_seed":
  db.exec("insert into items (id, source) values (1, 'root')")
"#,
    )
    .expect("write root entry");
    fs::write(
        dep_dir.join("main.fuse"),
        r#"requires db

migration "001_seed":
  db.exec("insert into items (id, source) values (2, 'auth')")
"#,
    )
    .expect("write dep entry");

    let output = run_migrate(&dir.join("main.fuse"), &db_url, 2);
    assert!(
        output.status.success(),
        "expected migration success, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let verify_db = Db::open_with_pool(&db_url, 1).expect("open verify db");
    let item_rows = verify_db
        .query("select count(*) as c from items")
        .expect("query items");
    assert_eq!(scalar_i64(&item_rows, "c"), 2);

    let root_rows = verify_db
        .query(
            "select count(*) as c from __fuse_migrations where package = '' and name = '001_seed'",
        )
        .expect("query root migration history");
    assert_eq!(scalar_i64(&root_rows, "c"), 1);

    let dep_rows = verify_db
        .query(
            "select count(*) as c from __fuse_migrations where package = 'auth' and name = '001_seed'",
        )
        .expect("query dep migration history");
    assert_eq!(scalar_i64(&dep_rows, "c"), 1);

    let _ = fs::remove_dir_all(dir);
}
