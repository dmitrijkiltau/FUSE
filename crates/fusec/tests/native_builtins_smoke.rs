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
fn native_builtins_smoke() {
    unsafe {
        std::env::set_var("FUSE_NATIVE_TEST", "on");
        std::env::set_var("FUSE_NATIVE_INT", "42");
        std::env::set_var("FUSE_NATIVE_FLOAT", "3.5");
        std::env::set_var("FUSE_NATIVE_BOOL", "TrUe");
    }
    let native = load_native_example("native_builtins.fuse");
    let mut vm = NativeVm::new(&native);

    let env_val = vm
        .call_function("env_present", vec![])
        .expect("env_present failed");
    assert!(vm.has_jit_function("env_present"));
    match env_val {
        Value::String(text) => assert_eq!(text, "on"),
        other => panic!("unexpected env_present value: {other:?}"),
    }

    let env_int = vm
        .call_function("env_int_present", vec![])
        .expect("env_int_present failed");
    assert!(vm.has_jit_function("env_int_present"));
    match env_int {
        Value::Int(value) => assert_eq!(value, 42),
        other => panic!("unexpected env_int_present value: {other:?}"),
    }

    let env_float = vm
        .call_function("env_float_present", vec![])
        .expect("env_float_present failed");
    assert!(vm.has_jit_function("env_float_present"));
    match env_float {
        Value::Float(value) => assert!((value - 3.5).abs() < 1e-9),
        other => panic!("unexpected env_float_present value: {other:?}"),
    }

    let env_bool = vm
        .call_function("env_bool_present", vec![])
        .expect("env_bool_present failed");
    assert!(vm.has_jit_function("env_bool_present"));
    match env_bool {
        Value::Bool(value) => assert!(value),
        other => panic!("unexpected env_bool_present value: {other:?}"),
    }

    let assert_ok = vm
        .call_function("assert_ok", vec![])
        .expect("assert_ok failed");
    assert!(vm.has_jit_function("assert_ok"));
    match assert_ok {
        Value::String(text) => assert_eq!(text, "ok"),
        other => panic!("unexpected assert_ok value: {other:?}"),
    }

    let log_demo = vm
        .call_function("log_demo", vec![])
        .expect("log_demo failed");
    assert!(vm.has_jit_function("log_demo"));
    match log_demo {
        Value::String(text) => assert_eq!(text, "ok"),
        other => panic!("unexpected log_demo value: {other:?}"),
    }

    let assert_fail = vm.call_function("assert_fail", vec![]);
    assert!(vm.has_jit_function("assert_fail"));
    let err = assert_fail.expect_err("assert_fail should return an error");
    assert_eq!(err, "assert failed: boom");
}
