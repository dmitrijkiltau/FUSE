use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn run_program(backend: &str, source: &str) -> std::process::Output {
    let program_path = write_temp_file("fuse_bool_compare_runtime", "fuse", source);
    let exe = env!("CARGO_BIN_EXE_fusec");
    Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&program_path)
        .output()
        .expect("failed to run fusec")
}

#[test]
fn bool_equality_and_inequality_work_across_backends() {
    let program = r#"
app "BoolCompare":
  let a = true
  let b = false
  print(a == true)
  print(a != false)
  print(b == false)
  print(b != true)
"#;

    for backend in ["ast", "native"] {
        let output = run_program(backend, program);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "true\ntrue\ntrue\ntrue", "{backend} stdout");
    }
}

#[test]
fn native_allows_nested_struct_field_access() {
    let program = r#"
type Inner:
  value: String

type Outer:
  inner: Inner

fn inner_value(outer: Outer) -> String:
  return outer.inner.value

app "NestedField":
  let outer = Outer(inner=Inner(value="ok"))
  print(inner_value(outer))
"#;

    let output = run_program("native", program);
    assert!(
        output.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "ok", "native stdout");
}

#[test]
fn native_compiles_optional_function_called_with_null_and_string_args() {
    let program = r#"
fn render_shell(token: String?, user_id: String?) -> String:
  let session_token = token ?? ""
  if session_token == "":
    return "anon"
  let uid = user_id ?? "unknown"
  return uid

fn from_root() -> String:
  return render_shell(null, null)

fn from_session(token: String) -> String:
  return render_shell(token, "u1")

app "OptionalCallKinds":
  print(from_root())
  print(from_session("session-1"))
"#;

    let output = run_program("native", program);
    assert!(
        output.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "anon\nu1", "native stdout");
}
