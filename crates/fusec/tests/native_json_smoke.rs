use std::path::PathBuf;

use fuse_rt::json as rt_json;

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
fn native_json_smoke() {
    let native = load_native_example("native_json.fuse");
    let mut vm = NativeVm::new(&native);

    let encoded = vm
        .call_function("encode_demo", vec![])
        .expect("encode_demo failed");
    assert!(vm.has_jit_function("encode_demo"));
    let encoded_text = match encoded {
        Value::String(text) => text,
        other => panic!("unexpected encode_demo value: {other:?}"),
    };
    let parsed = rt_json::decode(&encoded_text).expect("encoded json invalid");
    match parsed {
        rt_json::JsonValue::Object(map) => {
            assert!(matches!(map.get("a"), Some(rt_json::JsonValue::Number(n)) if *n == 1.0));
            assert!(matches!(map.get("b"), Some(rt_json::JsonValue::Bool(true))));
        }
        other => panic!("unexpected encoded json: {other:?}"),
    }

    let decoded = vm
        .call_function("decode_demo", vec![])
        .expect("decode_demo failed");
    assert!(vm.has_jit_function("decode_demo"));
    match decoded {
        Value::Map(map) => {
            assert!(matches!(map.get("a"), Some(Value::Int(1))));
            assert!(matches!(map.get("b"), Some(Value::Int(2))));
        }
        other => panic!("unexpected decode_demo value: {other:?}"),
    }

    let roundtrip = vm
        .call_function("roundtrip_demo", vec![])
        .expect("roundtrip_demo failed");
    assert!(vm.has_jit_function("roundtrip_demo"));
    let roundtrip_text = match roundtrip {
        Value::String(text) => text,
        other => panic!("unexpected roundtrip_demo value: {other:?}"),
    };
    let parsed = rt_json::decode(&roundtrip_text).expect("roundtrip json invalid");
    match parsed {
        rt_json::JsonValue::Object(map) => {
            assert!(matches!(map.get("a"), Some(rt_json::JsonValue::Number(n)) if *n == 1.0));
            assert!(matches!(map.get("b"), Some(rt_json::JsonValue::Number(n)) if *n == 2.0));
        }
        other => panic!("unexpected roundtrip json: {other:?}"),
    }
}
