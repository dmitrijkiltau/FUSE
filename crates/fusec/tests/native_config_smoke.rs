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
fn native_config_smoke() {
    unsafe {
        std::env::set_var("APP_GREETING", "Hi");
    }
    let native = load_native_example("project_demo.fuse");
    let mut vm = NativeVm::new(&native);
    let out = vm
        .call_function("greet", vec![Value::String("Ada".to_string())])
        .expect("greet failed");
    assert!(vm.has_jit_function("greet"));
    match out {
        Value::String(text) => assert_eq!(text, "Hi, Ada!"),
        other => panic!("unexpected greet value: {other:?}"),
    }
}
