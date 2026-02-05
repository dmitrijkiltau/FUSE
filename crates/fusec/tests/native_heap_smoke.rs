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
fn native_heap_literals_smoke() {
    let native = load_native_example("native_heap_literals.fuse");
    let mut vm = NativeVm::new(&native);

    let list = vm
        .call_function("make_list", vec![])
        .expect("native list call failed");
    assert!(vm.has_jit_function("make_list"), "expected JIT for make_list");
    match list {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            match &items[0] {
                Value::String(text) => assert_eq!(text, "alpha"),
                other => panic!("unexpected list item: {other:?}"),
            }
            match &items[1] {
                Value::String(text) => assert_eq!(text, "beta"),
                other => panic!("unexpected list item: {other:?}"),
            }
        }
        other => panic!("unexpected list value: {other:?}"),
    }

    let map = vm
        .call_function("make_map", vec![])
        .expect("native map call failed");
    assert!(vm.has_jit_function("make_map"), "expected JIT for make_map");
    match map {
        Value::Map(items) => {
            assert_eq!(items.len(), 2);
            match items.get("greeting") {
                Some(Value::String(text)) => assert_eq!(text, "hi"),
                other => panic!("unexpected greeting value: {other:?}"),
            }
            match items.get("target") {
                Some(Value::String(text)) => assert_eq!(text, "world"),
                other => panic!("unexpected target value: {other:?}"),
            }
        }
        other => panic!("unexpected map value: {other:?}"),
    }

    let user = vm
        .call_function("make_user", vec![])
        .expect("native make_user call failed");
    assert!(vm.has_jit_function("make_user"), "expected JIT for make_user");

    let role = vm
        .call_function("user_role", vec![user])
        .expect("native user_role call failed");
    assert!(vm.has_jit_function("user_role"), "expected JIT for user_role");
    match role {
        Value::String(text) => assert_eq!(text, "dev"),
        other => panic!("unexpected role value: {other:?}"),
    }

    let color = vm
        .call_function("make_color", vec![])
        .expect("native make_color call failed");
    assert!(vm.has_jit_function("make_color"), "expected JIT for make_color");
    match color {
        Value::Enum {
            name,
            variant,
            payload,
        } => {
            assert_eq!(name, "Color");
            assert_eq!(variant, "Rgb");
            assert_eq!(payload.len(), 3);
        }
        other => panic!("unexpected color value: {other:?}"),
    }

    let boxed = vm
        .call_function("make_boxed", vec![])
        .expect("native make_boxed call failed");
    assert!(vm.has_jit_function("make_boxed"), "expected JIT for make_boxed");
    match boxed {
        Value::Boxed(inner) => match &*inner.borrow() {
            Value::String(text) => assert_eq!(text, "boxed"),
            other => panic!("unexpected boxed inner value: {other:?}"),
        },
        other => panic!("unexpected boxed value: {other:?}"),
    }
}
