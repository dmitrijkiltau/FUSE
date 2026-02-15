use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::interp::{Interpreter, Value};
use fusec::native::{NativeVm, compile_registry};
use fusec::vm::Vm;

fn temp_project_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("fuse_module_scope_{tag}_{nanos}"));
    dir
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, contents).expect("write source file");
}

fn as_string(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => panic!("expected String, got {other:?}"),
    }
}

#[test]
fn module_scoped_functions_work_across_backends() {
    let dir = temp_project_dir("parity");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("a.fuse"),
        r#"
fn greet() -> String:
  return "a"
"#,
    );
    write_file(
        &dir.join("b.fuse"),
        r#"
fn greet() -> String:
  return "b"
"#,
    );
    write_file(
        &main_path,
        r#"
import A from "./a"
import B from "./b"
import { greet } from "./a"

fn main() -> String:
  return A.greet() + "|" + B.greet() + "|" + greet()
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let mut interp = Interpreter::with_registry(&registry);
    let ast = interp
        .call_function_with_named_args("main", &HashMap::new())
        .expect("ast call failed");

    let ir = fusec::ir::lower::lower_registry(&registry).expect("vm lowering failed");
    let mut vm = Vm::new(&ir);
    let vm_value = vm.call_function("main", vec![]).expect("vm call failed");

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![])
        .expect("native call failed");

    assert_eq!(as_string(ast), "a|b|a");
    assert_eq!(as_string(vm_value), "a|b|a");
    assert_eq!(as_string(native_value), "a|b|a");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn duplicate_named_imports_report_loader_diagnostics() {
    let dir = temp_project_dir("duplicate_import");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("a.fuse"),
        r#"
fn greet() -> String:
  return "a"
"#,
    );
    write_file(
        &dir.join("b.fuse"),
        r#"
fn greet() -> String:
  return "b"
"#,
    );
    write_file(
        &main_path,
        r#"
import { greet } from "./a"
import { greet } from "./b"
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read root source");
    let (_registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    let messages: Vec<String> = diags.iter().map(|diag| diag.message.clone()).collect();
    assert!(
        messages.iter().any(|msg| msg == "duplicate import greet"),
        "missing duplicate import diagnostic: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg == "previous import of greet here"),
        "missing previous import diagnostic: {messages:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_args_bind_to_root_main_even_when_dependencies_define_main() {
    let dir = temp_project_dir("cli_root_main");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("dep.fuse"),
        r#"
fn main(name: String):
  print("dep ${name}")
"#,
    );
    write_file(
        &main_path,
        r#"
import Dep from "./dep"

fn main(name: String):
  print("root ${name}")
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fusec");
    for backend in ["ast", "vm", "native"] {
        let output = Command::new(exe)
            .arg("--run")
            .arg("--backend")
            .arg(backend)
            .arg(&main_path)
            .arg("--")
            .arg("--name")
            .arg("Codex")
            .output()
            .expect("run fusec");
        assert!(
            output.status.success(),
            "{backend} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("root Codex"),
            "expected root main output for {backend}, got: {stdout}"
        );
        assert!(
            !stdout.contains("dep Codex"),
            "dependency main should not run for {backend}, got: {stdout}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}
