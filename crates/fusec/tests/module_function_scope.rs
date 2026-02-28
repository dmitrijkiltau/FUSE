use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::interp::{Interpreter, Value};
use fusec::native::{NativeVm, compile_registry};

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

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![])
        .expect("native call failed");

    assert_eq!(as_string(ast), "a|b|a");
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
    for backend in ["ast", "native"] {
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

#[test]
fn root_prefix_imports_resolve_from_manifest_root_across_backends() {
    let dir = temp_project_dir("root_prefix");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_file(
        &dir.join("fuse.toml"),
        r#"
[package]
entry = "src/main.fuse"
app = "Demo"
"#,
    );
    let main_path = dir.join("src").join("main.fuse");
    write_file(
        &dir.join("lib").join("greetings.fuse"),
        r#"
fn greet() -> String:
  return "root"
"#,
    );
    write_file(
        &main_path,
        r#"
import Greetings from "root:lib/greetings"

fn main() -> String:
  return Greetings.greet()
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

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![])
        .expect("native call failed");

    assert_eq!(as_string(ast), "root");
    assert_eq!(as_string(native_value), "root");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn capability_leakage_across_module_boundaries_is_reported() {
    let dir = temp_project_dir("capability_leak");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("auth.fuse"),
        r#"
requires db

fn lookup() -> Int:
  db.exec("create table if not exists users (id int)")
  return 1
"#,
    );
    write_file(
        &main_path,
        r#"
import Auth from "./auth"

fn main() -> Int:
  return Auth.lookup()
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected loader diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    let messages: Vec<String> = sema_diags.into_iter().map(|diag| diag.message).collect();
    assert!(
        messages.iter().any(|msg| msg
            == "call to Auth.lookup leaks capability db; add `requires db` at module top-level"),
        "missing capability leakage diagnostic: {messages:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn module_calls_with_declared_capability_pass() {
    let dir = temp_project_dir("capability_ok");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("auth.fuse"),
        r#"
requires db

fn lookup() -> Int:
  db.exec("create table if not exists users (id int)")
  return 1
"#,
    );
    write_file(
        &main_path,
        r#"
requires db

import Auth from "./auth"

fn main() -> Int:
  return Auth.lookup()
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected loader diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cross_module_error_domain_leakage_is_rejected() {
    let dir = temp_project_dir("error_domain_leak");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("auth.fuse"),
        r#"
type AuthError:
  message: String

fn lookup() -> Int!AuthError:
  return null ?! AuthError(message="missing")
"#,
    );
    write_file(
        &main_path,
        r#"
import Auth from "./auth"

type ApiError:
  message: String

fn main() -> Int!ApiError:
  return Auth.lookup()
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected loader diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    let messages: Vec<String> = sema_diags.into_iter().map(|diag| diag.message).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg == "type mismatch: expected Int!ApiError, found Int!AuthError"),
        "missing error-domain leakage diagnostic: {messages:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn strict_architecture_rejects_unused_capability_declarations() {
    let dir = temp_project_dir("strict_cap_purity");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &main_path,
        r#"
requires db

fn main() -> Int:
  return 1
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected loader diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry_with_options(
        &registry,
        fusec::sema::AnalyzeOptions {
            strict_architecture: true,
        },
    );
    let messages: Vec<String> = sema_diags.into_iter().map(|diag| diag.message).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("strict architecture: capability purity violation")),
        "missing strict capability purity diagnostic: {messages:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn strict_architecture_rejects_cross_layer_import_cycles() {
    let dir = temp_project_dir("strict_layer_cycle");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_file(
        &dir.join("src").join("main.fuse"),
        r#"
import Api from "./api/entry"

fn main() -> String:
  return Api.value()
"#,
    );
    write_file(
        &dir.join("src").join("api").join("entry.fuse"),
        r#"
import Ui from "../ui/screen"

fn value() -> String:
  return Ui.value()
"#,
    );
    write_file(
        &dir.join("src").join("api").join("helper.fuse"),
        r#"
fn value() -> String:
  return "ok"
"#,
    );
    write_file(
        &dir.join("src").join("ui").join("screen.fuse"),
        r#"
import ApiHelper from "../api/helper"

fn value() -> String:
  return ApiHelper.value()
"#,
    );

    let main_path = dir.join("src").join("main.fuse");
    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected loader diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry_with_options(
        &registry,
        fusec::sema::AnalyzeOptions {
            strict_architecture: true,
        },
    );
    let messages: Vec<String> = sema_diags.into_iter().map(|diag| diag.message).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("strict architecture: cross-layer import cycle detected")),
        "missing strict cross-layer cycle diagnostic: {messages:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn strict_architecture_rejects_mixed_error_domain_modules() {
    let dir = temp_project_dir("strict_error_isolation");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("auth_errors.fuse"),
        r#"
type AuthError:
  message: String
"#,
    );
    write_file(
        &dir.join("db_errors.fuse"),
        r#"
type DbError:
  message: String
"#,
    );
    write_file(
        &main_path,
        r#"
import { AuthError } from "./auth_errors"
import { DbError } from "./db_errors"

fn main() -> Int!AuthError!DbError:
  return 1
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected loader diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry_with_options(
        &registry,
        fusec::sema::AnalyzeOptions {
            strict_architecture: true,
        },
    );
    let messages: Vec<String> = sema_diags.into_iter().map(|diag| diag.message).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("strict architecture: error domain isolation violation")),
        "missing strict error-domain isolation diagnostic: {messages:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}
