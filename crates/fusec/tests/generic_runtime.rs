use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::interp::{Interpreter, Value};
use fusec::native::{NativeVm, compile_registry};

fn temp_project_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("fuse_generic_runtime_{tag}_{nanos}"));
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

// ---------------------------------------------------------------------------
// Test 1: generic identity function in both backends
// ---------------------------------------------------------------------------

#[test]
fn generic_identity_runs_in_ast_and_native_backends() {
    let dir = temp_project_dir("identity");
    let main_path = dir.join("main.fuse");
    write_file(
        &main_path,
        r#"
fn identity<T>(x: String) -> String:
  return x

fn main() -> String:
  return identity<String>("hello")
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected load diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let mut interp = Interpreter::with_registry(&registry);
    let ast_value = interp
        .call_function_with_named_args("main", &HashMap::new())
        .expect("ast call failed");

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![])
        .expect("native call failed");

    assert_eq!(as_string(ast_value), "hello");
    assert_eq!(as_string(native_value), "hello");

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Test 2: generic function that dispatches through an interface
// ---------------------------------------------------------------------------

#[test]
fn generic_interface_dispatch_runs_in_ast_and_native_backends() {
    let dir = temp_project_dir("interface");
    let main_path = dir.join("main.fuse");
    write_file(
        &main_path,
        r#"
interface Encodable:
  fn encode() -> String
  fn from_text(s: String) -> Self

type User:
  name: String

impl Encodable for User:
  fn encode() -> String:
    return self.name
  fn from_text(s: String) -> Self:
    return User(name=s)

fn round_trip<T>(text: String) -> String where T: Encodable:
  let decoded = T.from_text(text)
  return decoded.encode()

fn main() -> String:
  return round_trip<User>("ada")
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected load diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let mut interp = Interpreter::with_registry(&registry);
    let ast_value = interp
        .call_function_with_named_args("main", &HashMap::new())
        .expect("ast call failed");

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![])
        .expect("native call failed");

    assert_eq!(as_string(ast_value), "ada");
    assert_eq!(as_string(native_value), "ada");

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Test 3: cross-module generic function call
// ---------------------------------------------------------------------------

#[test]
fn cross_module_generic_call_runs_in_ast_and_native_backends() {
    let dir = temp_project_dir("cross");
    let main_path = dir.join("main.fuse");

    write_file(
        &dir.join("utils.fuse"),
        r#"
fn wrap<T>(value: String) -> String:
  return "wrapped:" + value
"#,
    );

    write_file(
        &main_path,
        r#"
import Utils from "./utils"

fn main() -> String:
  return Utils.wrap<String>("hello")
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected load diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let mut interp = Interpreter::with_registry(&registry);
    let ast_value = interp
        .call_function_with_named_args("main", &HashMap::new())
        .expect("ast call failed");

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![])
        .expect("native call failed");

    assert_eq!(as_string(ast_value), "wrapped:hello");
    assert_eq!(as_string(native_value), "wrapped:hello");

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Test 4: generic with multiple type args
// ---------------------------------------------------------------------------

#[test]
fn generic_multi_type_arg_runs_in_ast_and_native_backends() {
    let dir = temp_project_dir("multi");
    let main_path = dir.join("main.fuse");
    write_file(
        &main_path,
        r#"
fn combine<A, B>(a: String, b: String) -> String:
  return a + "|" + b

fn main() -> String:
  return combine<String, String>("foo", "bar")
"#,
    );

    let src = fs::read_to_string(&main_path).expect("read source");
    let (registry, diags) = fusec::load_program_with_modules(&main_path, &src);
    assert!(diags.is_empty(), "unexpected load diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );

    let mut interp = Interpreter::with_registry(&registry);
    let ast_value = interp
        .call_function_with_named_args("main", &HashMap::new())
        .expect("ast call failed");

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![])
        .expect("native call failed");

    assert_eq!(as_string(ast_value), "foo|bar");
    assert_eq!(as_string(native_value), "foo|bar");

    let _ = fs::remove_dir_all(&dir);
}
