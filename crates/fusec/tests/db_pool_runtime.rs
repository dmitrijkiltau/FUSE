use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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
    path.push(format!("fuse_db_pool_runtime_{stamp}.sqlite"));
    format!("sqlite://{}", path.display())
}

fn empty_config_path() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("/tmp/fuse_no_config_{stamp}")
}

fn run_program(backend: &str, path: &PathBuf, db_url: &str, pool_size: Option<&str>) -> Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(path)
        .env("FUSE_DB_URL", db_url)
        .env("FUSE_CONFIG", empty_config_path())
        .env_remove("DATABASE_URL")
        .env_remove("FUSE_DB_POOL_SIZE")
        .env_remove("APP_DB_POOL_SIZE");
    if let Some(size) = pool_size {
        cmd.env("FUSE_DB_POOL_SIZE", size);
    }
    cmd.output().expect("failed to run fusec")
}

#[test]
fn db_pool_size_env_rejects_invalid_values_all_backends() {
    let program = r#"
fn main():
  db.exec("create table if not exists items (id integer)")

app "demo":
  main()
"#;
    let path = write_temp_program("fuse_db_pool_env_invalid", program);
    for backend in ["ast", "native"] {
        let output = run_program(backend, &path, &temp_db_url(), Some("0"));
        assert!(
            !output.status.success(),
            "backend={backend} expected failure, stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("FUSE_DB_POOL_SIZE"),
            "backend={backend} stderr={stderr}"
        );
    }
}

#[test]
fn db_pool_size_defaults_to_one_when_unset_all_backends() {
    let program = r#"
fn main():
  db.exec("create table if not exists items (id integer)")

app "demo":
  main()
"#;
    let path = write_temp_program("fuse_db_pool_default", program);
    for backend in ["ast", "native"] {
        let output = run_program(backend, &path, &temp_db_url(), None);
        assert!(
            output.status.success(),
            "backend={backend} stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn db_pool_size_app_config_fallback_rejects_invalid_value_all_backends() {
    let program = r#"
config App:
  dbPoolSize: Int = 0

fn main():
  db.exec("create table if not exists items (id integer)")

app "demo":
  main()
"#;
    let path = write_temp_program("fuse_db_pool_config_invalid", program);
    for backend in ["ast", "native"] {
        let output = run_program(backend, &path, &temp_db_url(), None);
        assert!(
            !output.status.success(),
            "backend={backend} expected failure, stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("App.dbPoolSize"),
            "backend={backend} stderr={stderr}"
        );
    }
}
