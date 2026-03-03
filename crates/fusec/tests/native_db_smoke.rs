use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::interp::Value;
use fusec::native::{NativeProgram, NativeVm};

fn example_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path
}

fn load_native_example(name: &str) -> NativeProgram {
    let path = example_path(name);
    let src = std::fs::read_to_string(&path).expect("failed to read example");
    let (registry, diags) = fusec::load_program_with_modules(&path, &src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    fusec::native::compile_registry(&registry).expect("native compile failed")
}

fn temp_db_url() -> String {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("fuse_native_db_{stamp}.sqlite"));
    format!("sqlite://{}", path.display())
}

fn assert_user_struct(value: &Value, expected_id: i64, expected_name: &str) {
    match value {
        Value::Struct { name, fields } => {
            assert_eq!(name, "User");
            match fields.get("id") {
                Some(Value::Int(v)) => assert_eq!(*v, expected_id),
                other => panic!("unexpected user.id value: {other:?}"),
            }
            match fields.get("name") {
                Some(Value::String(v)) => assert_eq!(v, expected_name),
                other => panic!("unexpected user.name value: {other:?}"),
            }
        }
        other => panic!("expected User struct, got {other:?}"),
    }
}

#[test]
fn native_db_smoke() {
    let url = temp_db_url();
    unsafe {
        std::env::set_var("FUSE_DB_URL", url);
    }
    let native = load_native_example("native_db.fuse");
    let mut vm = NativeVm::new(&native);

    let init = vm.call_function("db_init", vec![]).expect("db_init failed");
    assert!(vm.has_jit_function("db_init"));
    match init {
        Value::Int(value) => assert_eq!(value, 1),
        other => panic!("unexpected db_init value: {other:?}"),
    }

    let one = vm
        .call_function("db_one_name", vec![])
        .expect("db_one_name failed");
    assert!(vm.has_jit_function("db_one_name"));
    match one {
        Value::Map(map) => match map.get("name") {
            Some(Value::String(text)) => assert_eq!(text, "Ada"),
            other => panic!("unexpected db_one_name map value: {other:?}"),
        },
        other => panic!("unexpected db_one_name value: {other:?}"),
    }

    let list = vm
        .call_function("db_query_names", vec![])
        .expect("db_query_names failed");
    assert!(vm.has_jit_function("db_query_names"));
    match list {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            let first = items.get(0).expect("missing first row");
            let second = items.get(1).expect("missing second row");
            match first {
                Value::Map(map) => match map.get("name") {
                    Some(Value::String(text)) => assert_eq!(text, "Ada"),
                    other => panic!("unexpected first row name: {other:?}"),
                },
                other => panic!("unexpected first row: {other:?}"),
            }
            match second {
                Value::Map(map) => match map.get("name") {
                    Some(Value::String(text)) => assert_eq!(text, "Bob"),
                    other => panic!("unexpected second row name: {other:?}"),
                },
                other => panic!("unexpected second row: {other:?}"),
            }
        }
        other => panic!("unexpected db_query_names value: {other:?}"),
    }

    let upsert = vm
        .call_function("db_upsert_name", vec![])
        .expect("db_upsert_name failed");
    assert!(vm.has_jit_function("db_upsert_name"));
    match upsert {
        Value::Map(map) => match map.get("name") {
            Some(Value::String(text)) => assert_eq!(text, "Bobby"),
            other => panic!("unexpected db_upsert_name map value: {other:?}"),
        },
        other => panic!("unexpected db_upsert_name value: {other:?}"),
    }

    let typed_list = vm
        .call_function("db_query_typed_users", vec![])
        .expect("db_query_typed_users failed");
    assert!(vm.has_jit_function("db_query_typed_users"));
    match typed_list {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            assert_user_struct(&items[0], 1, "Ada");
            assert_user_struct(&items[1], 2, "Bobby");
        }
        other => panic!("unexpected db_query_typed_users value: {other:?}"),
    }

    let typed_one = vm
        .call_function("db_one_typed_user", vec![Value::Int(1)])
        .expect("db_one_typed_user failed");
    assert!(vm.has_jit_function("db_one_typed_user"));
    assert_user_struct(&typed_one, 1, "Ada");

    let typed_missing = vm
        .call_function("db_one_typed_user", vec![Value::Int(999)])
        .expect("db_one_typed_user missing failed");
    assert!(matches!(typed_missing, Value::Null));
}
