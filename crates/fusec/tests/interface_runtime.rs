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
    dir.push(format!("fuse_interface_runtime_{tag}_{nanos}"));
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
fn interface_dispatch_runs_in_ast_and_native_backends() {
    let dir = temp_project_dir("parity");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(
        &dir.join("models.fuse"),
        r#"
interface Codec:
  fn encode() -> String
  fn from_text(text: String) -> Self
  fn duplicate() -> String

type User:
  name: String

impl Codec for User:
  fn encode() -> String:
    return self.name

  fn from_text(text: String) -> Self:
    return User(name=text)

  fn duplicate() -> String:
    return Self.from_text(self.name + self.name).encode()
"#,
    );
    write_file(
        &main_path,
        r#"
import Models from "./models"
import { User } from "./models"

fn main(user: User) -> String:
  let first = user.encode()
  let second = user.duplicate()
  let third = User.from_text("mia").encode()
  let fourth = Models.User.from_text("noah").duplicate()
  return first + "|" + second + "|" + third + "|" + fourth
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

    let user = Value::Struct {
        name: "User".to_string(),
        fields: HashMap::from([("name".to_string(), Value::String("ada".to_string()))]),
    };

    let mut interp = Interpreter::with_registry(&registry);
    let ast_value = interp
        .call_function_with_named_args("main", &HashMap::from([("user".to_string(), user.clone())]))
        .expect("ast call failed");

    let native = compile_registry(&registry).expect("native lowering failed");
    let mut native_vm = NativeVm::new(&native);
    let native_value = native_vm
        .call_function("main", vec![user])
        .expect("native call failed");

    assert_eq!(as_string(ast_value), "ada|adaada|mia|noahnoah");
    assert_eq!(as_string(native_value), "ada|adaada|mia|noahnoah");

    let _ = fs::remove_dir_all(&dir);
}
