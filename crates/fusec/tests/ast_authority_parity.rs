use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::interp::{Interpreter, Value};
use fusec::native::{NativeVm, compile_registry};
use fusec::vm::Vm;

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{counter}-{}", std::process::id())
}

fn write_temp_file(tag: &str, ext: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "fuse_ast_authority_{tag}_{}.{}",
        unique_suffix(),
        ext
    ));
    fs::write(&path, contents).expect("write temp file");
    path
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, contents).expect("write file");
}

fn temp_project_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("fuse_ast_authority_{tag}_{}", unique_suffix()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn run_program(backend: &str, source: &str, envs: &[(&str, &str)]) -> Output {
    let path = write_temp_file("program", "fuse", source);
    run_program_path(backend, &path, envs)
}

fn run_program_path(backend: &str, path: &Path, envs: &[(&str, &str)]) -> Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run").arg("--backend").arg(backend).arg(path);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("run fusec")
}

#[test]
fn config_structured_env_overrides_are_parity_gated() {
    let program = r#"
type User:
  name: String
  age: Int(0..120)

config App:
  names: List<String> = ["Default"]
  labels: Map<String, Int> = {"x": 1}
  profile: User = User(name="anon", age=1)

app "demo":
  print(App.names[0])
  print(App.labels["x"])
  print(App.profile.name)
"#;
    let envs = [
        ("APP_NAMES", r#"["Ada"]"#),
        ("APP_LABELS", r#"{"x":5}"#),
        ("APP_PROFILE", r#"{"name":"Bea","age":33}"#),
    ];

    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program, &envs);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines, vec!["Ada", "5", "Bea"], "{backend} stdout");
    }
}

#[test]
fn html_local_function_shadows_tag_builtin_across_backends() {
    let program = r#"
fn div() -> String:
  return "local"

app "demo":
  print(div())
"#;

    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program, &[]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "local",
            "{backend} stdout"
        );
    }
}

#[test]
fn html_imported_function_shadows_tag_builtin_across_backends() {
    let dir = temp_project_dir("import_shadow");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("dep.fuse"),
        r#"
fn div() -> String:
  return "imported"
"#,
    );
    write_file(
        &main_path,
        r#"
import { div } from "./dep"

app "demo":
  print(div())
"#,
    );

    for backend in ["ast", "vm", "native"] {
        let output = run_program_path(backend, &main_path, &[]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "imported",
            "{backend} stdout"
        );
    }

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn html_config_name_is_not_treated_as_tag_builtin() {
    let program = r#"
config div:
  title: String = "cfg"

app "demo":
  print(div.title)
  print(div())
"#;

    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program, &[]);
        assert!(
            !output.status.success(),
            "{backend} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("call target is not callable")
                || stderr.contains("unknown function div")
                || stderr.contains("native backend could not compile function"),
            "{backend} stderr: {stderr}"
        );
        assert!(!stderr.contains("<div"), "{backend} stderr: {stderr}");
    }
}

#[test]
fn public_call_apis_reject_extra_arguments_consistently() {
    let src = r#"
fn main(name: String) -> String:
  return name
"#;
    let path = write_temp_file("api_arity", "fuse", src);
    let (registry, diags) = fusec::load_program_with_modules(&path, src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let mut ast = Interpreter::with_registry(&registry);
    let mut named = HashMap::new();
    named.insert("name".to_string(), Value::String("Ada".to_string()));
    named.insert("extra".to_string(), Value::String("unused".to_string()));
    let ast_err = ast
        .call_function_with_named_args("main", &named)
        .expect_err("ast should reject unknown named arguments");
    assert!(
        ast_err.contains("unknown argument"),
        "unexpected ast error: {ast_err}"
    );

    let ir = fusec::ir::lower::lower_registry(&registry).expect("vm lowering failed");
    let mut vm = Vm::new(&ir);
    let vm_err = vm
        .call_function(
            "main",
            vec![
                Value::String("Ada".to_string()),
                Value::String("unused".to_string()),
            ],
        )
        .expect_err("vm should reject extra positional arguments");
    assert!(
        vm_err.contains("invalid call to"),
        "unexpected vm error: {vm_err}"
    );

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_err = native_vm
        .call_function(
            "main",
            vec![
                Value::String("Ada".to_string()),
                Value::String("unused".to_string()),
            ],
        )
        .expect_err("native should reject extra positional arguments");
    assert!(
        native_err.contains("invalid call to"),
        "unexpected native error: {native_err}"
    );
}

#[test]
fn function_defaults_use_prior_params_across_backends() {
    let program = r#"
fn greet(prefix: String, name: String = prefix + " Ada", full: String = name + "!") -> String:
  return full

app "demo":
  print(greet("Hello"))
  print(greet("Hi", "Bea"))
  print(greet("Yo", "Cid", "done"))
"#;

    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program, &[]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            lines,
            vec!["Hello Ada!", "Bea!", "done"],
            "{backend} stdout"
        );
    }
}

#[test]
fn imported_function_defaults_apply_across_backends() {
    let dir = temp_project_dir("import_defaults");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("dep.fuse"),
        r#"
fn greet(prefix: String, name: String = prefix + " Ada", full: String = name + "!") -> String:
  return full
"#,
    );
    write_file(
        &main_path,
        r#"
import { greet } from "./dep"

app "demo":
  print(greet("Hello"))
  print(greet("Hi", "Bea"))
  print(greet("Yo", "Cid", "done"))
"#,
    );

    for backend in ["ast", "vm", "native"] {
        let output = run_program_path(backend, &main_path, &[]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            lines,
            vec!["Hello Ada!", "Bea!", "done"],
            "{backend} stdout"
        );
    }

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn html_attr_shorthand_non_literal_rejected_across_backends() {
    let program = r#"
fn page(name: String) -> Html:
  return div(class=name):
    "hello"

app "demo":
  print(html.render(page("Ada")))
"#;

    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program, &[]);
        assert!(
            !output.status.success(),
            "{backend} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("html attribute shorthand only supports string literals"),
            "{backend} stderr: {stderr}"
        );
    }
}

#[test]
fn html_attr_shorthand_mixed_positional_rejected_across_backends() {
    let program = r#"
fn page() -> Html:
  return div({"class": "hero"}, id="main")

app "demo":
  print(html.render(page()))
"#;

    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program, &[]);
        assert!(
            !output.status.success(),
            "{backend} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot mix html attribute shorthand with positional arguments"),
            "{backend} stderr: {stderr}"
        );
    }
}
