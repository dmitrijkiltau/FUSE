use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_project_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("fuse_dotenv_test_{nanos}"));
    dir
}

#[test]
fn loads_dotenv_from_manifest_dir() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");

    let program = r#"
config App:
  port: Int = 0

app "Demo":
  print(App.port)
"#;
    fs::write(dir.join("main.fuse"), program).expect("write main.fuse");

    let dotenv = "APP_PORT=4242\n";
    fs::write(dir.join(".env"), dotenv).expect("write .env");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse");

    if !output.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "4242");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn accepts_manifest_dir_positional() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");

    let program = r#"
app "Demo":
  print("ok")
"#;
    fs::write(dir.join("main.fuse"), program).expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("run")
        .arg(&dir)
        .output()
        .expect("run fuse");

    if !output.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "ok");

    let _ = fs::remove_dir_all(&dir);
}
