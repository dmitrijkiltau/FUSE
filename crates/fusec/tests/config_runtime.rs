use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
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

fn run_program_backend(backend: &str, program_path: &PathBuf, envs: &[(&str, &str)]) -> Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(program_path);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("failed to run fusec")
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
fn config_defaults_apply_before_validation_all_backends() {
    let program = r#"
config TestCfg:
  code: String(3..3) = "abc"

app "demo":
  print(TestCfg.code)
"#;
    let program_path = write_temp_file("fuse_config_default_validation_order", "fuse", program);
    for backend in ["ast", "native"] {
        let output = run_program_backend(backend, &program_path, &[]);
        assert!(
            output.status.success(),
            "backend={backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "abc",
            "backend={backend}"
        );
    }
}

#[test]
fn config_explicit_null_preserves_null_even_with_default_all_backends() {
    let program = r#"
config TestCfg:
  alias: String? = "anon"

app "demo":
  print(TestCfg.alias ?? "null")
"#;
    let program_path = write_temp_file("fuse_config_null_default_precedence", "fuse", program);
    for backend in ["ast", "native"] {
        let output = run_program_backend(
            backend,
            &program_path,
            &[("TEST_CFG_ALIAS", "null")],
        );
        assert!(
            output.status.success(),
            "backend={backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "null",
            "backend={backend}"
        );
    }
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

#[test]
fn config_structured_json_support_and_bytes_validation() {
    let program = r#"
config App:
  names: List<String> = ["Default"]
  labels: Map<String, Int> = {"a": 1}
  profile: User = User(name="anon", age=1)
  token: Bytes = "Zg=="

type User:
  name: String
  age: Int(0..120)

app "demo":
  print(App.names[0])
  print(App.labels["x"])
  print(App.profile.name)
  print(App.token)
"#;

    let config = r#"
[App]
names = "[\"Ada\"]"
labels = "{\"x\":5}"
profile = "{\"name\":\"Bea\",\"age\":33}"
token = "Zm9v"
"#;

    let program_path = write_temp_file("fuse_config_structured", "fuse", program);
    let config_path = write_temp_file("fuse_config_structured", "toml", config);

    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&program_path)
        .env("FUSE_CONFIG", &config_path)
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["Ada", "5", "Bea", "Zm9v"]);

    let bad = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&program_path)
        .env("FUSE_CONFIG", &config_path)
        .env("APP_TOKEN", "not_base64")
        .output()
        .expect("failed to run fusec");
    assert!(!bad.status.success(), "expected invalid bytes failure");
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("invalid Bytes (base64)"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_binding_supports_structured_json_values() {
    let program = r#"
fn main(names: List<String>, labels: Map<String, Int>, profile: User, token: Bytes):
  print(names[0])
  print(labels["k"])
  print(profile.name)
  print(token)

type User:
  name: String
"#;

    let program_path = write_temp_file("fuse_cli_structured", "fuse", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&program_path)
        .arg("--")
        .arg("--names=[\"Nia\"]")
        .arg("--labels={\"k\":7}")
        .arg("--profile={\"name\":\"Mia\"}")
        .arg("--token=YQ==")
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["Nia", "7", "Mia", "YQ=="]);

    let bad = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&program_path)
        .arg("--")
        .arg("--names=[\"Nia\"]")
        .arg("--labels={\"k\":7}")
        .arg("--profile={\"name\":\"Mia\"}")
        .arg("--token=not_base64")
        .output()
        .expect("failed to run fusec");
    assert!(!bad.status.success(), "expected invalid bytes failure");
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("invalid Bytes (base64)"),
        "stderr: {stderr}"
    );
}
