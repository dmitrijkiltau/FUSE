use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fuse_rt::json::{self, JsonValue};

fn write_temp_file(name: &str, ext: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("{name}_{stamp}.{ext}"));
    fs::write(&path, contents).expect("failed to write temp file");
    path
}

#[test]
fn config_precedence_ast() {
    let program = r#"
config App:
  greeting: String = "DefaultGreet"
  who: String = "DefaultWho"
  role: String = "DefaultRole"

app "demo":
  print(App.greeting)
  print(App.who)
  print(App.role)
"#;

    let config = r#"
[App]
greeting = "FileGreet"
who = "FileWho"
"#;

    let program_path = write_temp_file("fuse_config_precedence", "fuse", program);
    let config_path = write_temp_file("fuse_config_precedence", "toml", config);

    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&program_path)
        .env("FUSE_CONFIG", &config_path)
        .env("APP_GREETING", "EnvGreet")
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["EnvGreet", "FileWho", "DefaultRole"]);
}

#[test]
fn config_validation_error_fields() {
    let program = r#"
config App:
  age: Int(0..10) = 1

app "demo":
  print(App.age)
"#;

    let program_path = write_temp_file("fuse_config_validation", "fuse", program);

    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&program_path)
        .env("APP_AGE", "99")
        .output()
        .expect("failed to run fusec");

    assert!(
        !output.status.success(),
        "expected failure, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    let json_text = stderr.strip_prefix("run error: ").unwrap_or(stderr).trim();
    let json_value = json::decode(json_text).expect("expected JSON validation error");

    let JsonValue::Object(root) = json_value else {
        panic!("expected JSON object");
    };
    let JsonValue::Object(err) = root.get("error").expect("missing error") else {
        panic!("expected error object");
    };
    let JsonValue::String(code) = err.get("code").expect("missing error code") else {
        panic!("expected error code");
    };
    assert_eq!(code, "validation_error");
    let JsonValue::Array(fields) = err.get("fields").expect("missing fields") else {
        panic!("expected fields array");
    };
    assert_eq!(fields.len(), 1);
    let JsonValue::Object(field) = &fields[0] else {
        panic!("expected field object");
    };
    let JsonValue::String(path) = field.get("path").expect("missing path") else {
        panic!("expected path string");
    };
    assert_eq!(path, "App.age");
    let JsonValue::String(code) = field.get("code").expect("missing field code") else {
        panic!("expected field code");
    };
    assert_eq!(code, "invalid_value");
}
