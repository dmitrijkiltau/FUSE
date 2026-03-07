use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::diag::Diag;
use fusec::interp::{Interpreter, Value};
use fusec::native::{NativeVm, compile_registry};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn temp_project_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("fuse_db_typed_query_{tag}_{nanos}"));
    dir
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, contents).expect("write source file");
}

fn temp_db_url(tag: &str) -> String {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("fuse_db_typed_query_{tag}_{nanos}.sqlite"));
    format!("sqlite://{}", path.display())
}

fn program_source() -> &'static str {
    r#"
requires db

type User:
  id: Int
  name: String

fn seed():
  db.exec("create table if not exists users (id integer primary key, name text not null)")
  db.exec("delete from users")
  db.from("users").insert(User(id=1, name="Ada")).exec()
  db.from("users").insert(User(id=2, name="Bob")).exec()
  db.from("users").insert(User(id=3, name="Cy")).exec()

fn all_users_typed() -> List<User>:
  return db.from("users").select(["id", "name"]).order_by("id", "asc").all<User>()

fn one_user_typed(id: Int) -> User?:
  return db.from("users").select(["id", "name"]).where("id", "=", id).one<User>()
"#
}

fn load_registry(main_path: &Path) -> fusec::loader::ModuleRegistry {
    let src = fs::read_to_string(main_path).expect("read root source");
    let (registry, diags) = fusec::load_program_with_modules(main_path, &src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    assert!(
        sema_diags.is_empty(),
        "unexpected sema diagnostics: {sema_diags:?}"
    );
    registry
}

fn analyze_registry_diags(src: &str, tag: &str) -> Vec<Diag> {
    let dir = temp_project_dir(tag);
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(&main_path, src);
    let root_src = fs::read_to_string(&main_path).expect("read root source");
    let (registry, load_diags) = fusec::load_program_with_modules(&main_path, &root_src);
    assert!(
        load_diags.is_empty(),
        "unexpected loader diagnostics: {load_diags:?}"
    );
    let (_analysis, sema_diags) = fusec::sema::analyze_registry(&registry);
    let _ = fs::remove_dir_all(&dir);
    sema_diags
}

fn expect_user_struct(value: &Value, expected_id: i64, expected_name: &str) {
    let Value::Struct { name, fields } = value else {
        panic!("expected User struct, got {value:?}");
    };
    assert_eq!(name, "User");
    match fields.get("id") {
        Some(Value::Int(v)) => assert_eq!(*v, expected_id),
        other => panic!("expected id Int({expected_id}), got {other:?}"),
    }
    match fields.get("name") {
        Some(Value::String(v)) => assert_eq!(v, expected_name),
        other => panic!("expected name String({expected_name}), got {other:?}"),
    }
}

#[test]
fn typed_query_results_in_ast_backend() {
    let _env_guard = ENV_LOCK.lock().expect("lock env guard");
    let dir = temp_project_dir("ast");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(&main_path, program_source());
    let registry = load_registry(&main_path);

    let db_url = temp_db_url("ast");
    unsafe {
        std::env::set_var("FUSE_DB_URL", &db_url);
    }
    let mut interp = Interpreter::with_registry(&registry);
    interp
        .call_function_with_named_args("seed", &HashMap::new())
        .expect("seed failed");

    let all = interp
        .call_function_with_named_args("all_users_typed", &HashMap::new())
        .expect("all_users_typed failed");
    match all {
        Value::List(items) => {
            assert_eq!(items.len(), 3);
            expect_user_struct(&items[0], 1, "Ada");
            expect_user_struct(&items[1], 2, "Bob");
            expect_user_struct(&items[2], 3, "Cy");
        }
        other => panic!("expected typed user list, got {other:?}"),
    }

    let mut args = HashMap::new();
    args.insert("id".to_string(), Value::Int(2));
    let one = interp
        .call_function_with_named_args("one_user_typed", &args)
        .expect("one_user_typed failed");
    expect_user_struct(&one, 2, "Bob");

    let mut missing_args = HashMap::new();
    missing_args.insert("id".to_string(), Value::Int(999));
    let missing = interp
        .call_function_with_named_args("one_user_typed", &missing_args)
        .expect("one_user_typed missing failed");
    assert!(matches!(missing, Value::Null));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn typed_query_results_in_native_backend() {
    let _env_guard = ENV_LOCK.lock().expect("lock env guard");
    let dir = temp_project_dir("native");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main_path = dir.join("main.fuse");
    write_file(&main_path, program_source());
    let registry = load_registry(&main_path);

    let db_url = temp_db_url("native");
    unsafe {
        std::env::set_var("FUSE_DB_URL", &db_url);
    }
    let native = compile_registry(&registry).expect("native lowering failed");
    let mut vm = NativeVm::new(&native);
    vm.call_function("seed", vec![]).expect("seed failed");

    let all = vm
        .call_function("all_users_typed", vec![])
        .expect("all_users_typed failed");
    match all {
        Value::List(items) => {
            assert_eq!(items.len(), 3);
            expect_user_struct(&items[0], 1, "Ada");
            expect_user_struct(&items[1], 2, "Bob");
            expect_user_struct(&items[2], 3, "Cy");
        }
        other => panic!("expected typed user list, got {other:?}"),
    }

    let one = vm
        .call_function("one_user_typed", vec![Value::Int(2)])
        .expect("one_user_typed failed");
    expect_user_struct(&one, 2, "Bob");

    let missing = vm
        .call_function("one_user_typed", vec![Value::Int(999)])
        .expect("one_user_typed missing failed");
    assert!(matches!(missing, Value::Null));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn typed_query_missing_select_reports_code_and_expected_fields() {
    let diags = analyze_registry_diags(
        r#"
requires db

type User:
  id: Int
  name: String

fn all_users() -> List<User>:
  return db.from("users").all<User>()
"#,
        "missing_select_diag",
    );
    assert_eq!(diags.len(), 1, "unexpected diagnostics: {diags:?}");
    assert_eq!(diags[0].code.as_deref(), Some("FUSE_TYPED_QUERY_SELECT"));
    assert_eq!(
        diags[0].message,
        "typed query requires select([...]) before one<T>()/all<T>(); expected User fields [id, name]"
    );
}

#[test]
fn typed_query_field_mismatch_reports_code_and_missing_fields() {
    let diags = analyze_registry_diags(
        r#"
requires db

type User:
  id: Int
  name: String

fn all_users() -> List<User>:
  return db.from("users").select(["id"]).all<User>()
"#,
        "field_mismatch_diag",
    );
    assert_eq!(diags.len(), 1, "unexpected diagnostics: {diags:?}");
    assert_eq!(
        diags[0].code.as_deref(),
        Some("FUSE_TYPED_QUERY_FIELD_MISMATCH")
    );
    assert_eq!(
        diags[0].message,
        "typed query projection for User does not match selected columns: missing [name]; expected fields [id, name]"
    );
}

#[test]
fn typed_query_non_type_result_target_reports_code() {
    let diags = analyze_registry_diags(
        r#"
requires db

fn all_users() -> List<String>:
  return db.from("users").select(["id"]).all<String>()
"#,
        "type_arg_diag",
    );
    assert_eq!(diags.len(), 1, "unexpected diagnostics: {diags:?}");
    assert_eq!(diags[0].code.as_deref(), Some("FUSE_TYPED_QUERY_TYPE_ARG"));
    assert_eq!(
        diags[0].message,
        "typed query result type must be a declared `type`, found String"
    );
}
