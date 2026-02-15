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

#[test]
fn native_bang_error_smoke() {
    let native = load_native_example("native_bang_error.fuse");
    let mut vm = NativeVm::new(&native);

    let ok = vm
        .call_function("bang_ok", vec![])
        .expect("native bang_ok call failed");
    assert!(vm.has_jit_function("bang_ok"), "expected JIT for bang_ok");
    match ok {
        Value::ResultOk(inner) => match inner.as_ref() {
            Value::String(text) => assert_eq!(text, "ok"),
            other => panic!("unexpected ok value: {other:?}"),
        },
        other => panic!("unexpected bang_ok value: {other:?}"),
    }

    let err = vm
        .call_function("bang_error", vec![])
        .expect("native bang_error call failed");
    assert!(
        vm.has_jit_function("bang_error"),
        "expected JIT for bang_error"
    );
    match err {
        Value::ResultErr(inner) => match inner.as_ref() {
            Value::Struct { name, fields } => {
                assert_eq!(name, "std.Error");
                match fields.get("message") {
                    Some(Value::String(text)) => assert_eq!(text, "boom"),
                    other => panic!("unexpected error message: {other:?}"),
                }
            }
            other => panic!("unexpected error value: {other:?}"),
        },
        other => panic!("unexpected bang_error value: {other:?}"),
    }

    let pass_ok = vm
        .call_function("bang_passthrough", vec![Value::String("alive".to_string())])
        .expect("native bang_passthrough ok failed");
    match pass_ok {
        Value::ResultOk(inner) => match inner.as_ref() {
            Value::String(text) => assert_eq!(text, "alive"),
            other => panic!("unexpected bang_passthrough ok value: {other:?}"),
        },
        other => panic!("unexpected bang_passthrough ok value: {other:?}"),
    }

    let pass_err = vm
        .call_function("bang_passthrough", vec![Value::Null])
        .expect("native bang_passthrough err failed");
    match pass_err {
        Value::ResultErr(inner) => match inner.as_ref() {
            Value::Struct { name, fields } => {
                assert_eq!(name, "std.Error");
                match fields.get("message") {
                    Some(Value::String(text)) => assert_eq!(text, "missing"),
                    other => panic!("unexpected bang_passthrough error message: {other:?}"),
                }
            }
            other => panic!("unexpected bang_passthrough err value: {other:?}"),
        },
        other => panic!("unexpected bang_passthrough err value: {other:?}"),
    }

    let coalesce_null = vm
        .call_function("coalesce_name", vec![Value::Null])
        .expect("native coalesce_name null failed");
    match coalesce_null {
        Value::String(text) => assert_eq!(text, "fallback"),
        other => panic!("unexpected coalesce null value: {other:?}"),
    }

    let coalesce_value = vm
        .call_function("coalesce_name", vec![Value::String("Ada".to_string())])
        .expect("native coalesce_name value failed");
    match coalesce_value {
        Value::String(text) => assert_eq!(text, "Ada"),
        other => panic!("unexpected coalesce value: {other:?}"),
    }

    let mut fields = std::collections::HashMap::new();
    fields.insert("name".to_string(), Value::String("Zoe".to_string()));
    let user = Value::Struct {
        name: "User".to_string(),
        fields,
    };
    let opt_some = vm
        .call_function("opt_name", vec![user])
        .expect("native opt_name some failed");
    match opt_some {
        Value::String(text) => assert_eq!(text, "Zoe"),
        other => panic!("unexpected opt_name some value: {other:?}"),
    }

    let opt_none = vm
        .call_function("opt_name", vec![Value::Null])
        .expect("native opt_name none failed");
    match opt_none {
        Value::String(text) => assert_eq!(text, "none"),
        other => panic!("unexpected opt_name none value: {other:?}"),
    }
}
