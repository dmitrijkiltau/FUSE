use std::path::PathBuf;

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

fn extract_validation(err: &Value) -> (&str, &str, &str) {
    let Value::Struct { name, fields } = err else {
        panic!("unexpected error value: {err:?}");
    };
    assert_eq!(name, "std.Error.Validation");
    let fields_list = match fields.get("fields") {
        Some(Value::List(items)) => items,
        other => panic!("unexpected validation fields: {other:?}"),
    };
    assert_eq!(fields_list.len(), 1);
    let Value::Struct { fields, .. } = &fields_list[0] else {
        panic!("unexpected validation field: {:?}", fields_list[0]);
    };
    let path = match fields.get("path") {
        Some(Value::String(text)) => text.as_str(),
        other => panic!("unexpected path: {other:?}"),
    };
    let code = match fields.get("code") {
        Some(Value::String(text)) => text.as_str(),
        other => panic!("unexpected code: {other:?}"),
    };
    let message = match fields.get("message") {
        Some(Value::String(text)) => text.as_str(),
        other => panic!("unexpected message: {other:?}"),
    };
    (path, code, message)
}

#[test]
fn native_validation_smoke() {
    let native = load_native_example("native_validation.fuse");
    let mut vm = NativeVm::new(&native);

    let ok = vm.call_function("ok_user", vec![]).expect("ok_user failed");
    assert!(vm.has_jit_function("ok_user"));
    match ok {
        Value::ResultOk(inner) => match inner.as_ref() {
            Value::Struct { name, .. } => assert_eq!(name, "User"),
            other => panic!("unexpected ok_user value: {other:?}"),
        },
        other => panic!("unexpected ok_user result: {other:?}"),
    }

    let bad_id = vm
        .call_function("bad_id", vec![])
        .expect("bad_id call failed");
    assert!(vm.has_jit_function("bad_id"));
    match bad_id {
        Value::ResultErr(inner) => {
            let (path, code, message) = extract_validation(inner.as_ref());
            assert_eq!(path, "User.id");
            assert_eq!(code, "invalid_value");
            assert_eq!(message, "expected non-empty Id");
        }
        other => panic!("unexpected bad_id result: {other:?}"),
    }

    let bad_email = vm
        .call_function("bad_email", vec![])
        .expect("bad_email call failed");
    assert!(vm.has_jit_function("bad_email"));
    match bad_email {
        Value::ResultErr(inner) => {
            let (path, code, message) = extract_validation(inner.as_ref());
            assert_eq!(path, "User.email");
            assert_eq!(code, "invalid_value");
            assert_eq!(message, "invalid email address");
        }
        other => panic!("unexpected bad_email result: {other:?}"),
    }

    let bad_age = vm
        .call_function("bad_age", vec![])
        .expect("bad_age call failed");
    assert!(vm.has_jit_function("bad_age"));
    match bad_age {
        Value::ResultErr(inner) => {
            let (path, code, message) = extract_validation(inner.as_ref());
            assert_eq!(path, "User.age");
            assert_eq!(code, "invalid_value");
            assert_eq!(message, "value 20 out of range 0..10");
        }
        other => panic!("unexpected bad_age result: {other:?}"),
    }
}
