use std::path::PathBuf;
use std::time::{Duration, Instant};

use fusec::interp::Value;
use fusec::native::{NativeProgram, NativeVm};
use fusec::vm::Vm;

fn example_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path
}

fn load_bench_artifacts() -> (fusec::ir::Program, NativeProgram) {
    let path = example_path("native_bench.fuse");
    let src = std::fs::read_to_string(&path).expect("failed to read benchmark example");
    let (registry, diags) = fusec::load_program_with_modules(&path, &src);
    assert!(
        diags.is_empty(),
        "unexpected diagnostics while loading benchmark example: {diags:?}"
    );
    let ir = fusec::ir::lower::lower_registry(&registry).expect("failed to lower benchmark");
    let native =
        fusec::native::compile_registry(&registry).expect("failed to compile native program");
    (ir, native)
}

fn call_once_vm(ir: &fusec::ir::Program, n: i64) -> Duration {
    let mut vm = Vm::new(ir);
    let start = Instant::now();
    let out = vm
        .call_function("main", vec![Value::Int(n)])
        .expect("vm call failed");
    assert!(
        matches!(out, Value::Int(_)),
        "vm returned unexpected value: {out:?}"
    );
    start.elapsed()
}

fn call_once_native(native: &NativeProgram, n: i64) -> Duration {
    let mut vm = NativeVm::new(native);
    let start = Instant::now();
    let out = vm
        .call_function("main", vec![Value::Int(n)])
        .expect("native call failed");
    assert!(
        vm.has_jit_function("main"),
        "expected JIT compilation for main"
    );
    assert!(
        matches!(out, Value::Int(_)),
        "native returned unexpected value: {out:?}"
    );
    start.elapsed()
}

fn env_budget_ms(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|raw| raw.parse::<u64>().ok())
}

fn check_budget(label: &str, duration: Duration, env: &str) {
    if let Some(limit_ms) = env_budget_ms(env) {
        let limit = Duration::from_millis(limit_ms);
        assert!(
            duration <= limit,
            "{label} exceeded budget: {:?} > {:?} (env {env}={limit_ms})",
            duration,
            limit
        );
    }
}

#[test]
fn native_bench_smoke() {
    let (ir, native) = load_bench_artifacts();

    let cold_vm = call_once_vm(&ir, 400_000);
    let cold_native = call_once_native(&native, 400_000);

    let mut warm_vm = Duration::ZERO;
    let mut warm_native = Duration::ZERO;
    for _ in 0..8 {
        warm_vm += call_once_vm(&ir, 120_000);
        warm_native += call_once_native(&native, 120_000);
    }
    let warm_native_avg = Duration::from_secs_f64(warm_native.as_secs_f64() / 8.0);

    eprintln!(
        "native bench smoke: cold vm={:?} native={:?}; warm vm={:?} native={:?}",
        cold_vm, cold_native, warm_vm, warm_native
    );

    check_budget("native cold-start", cold_native, "FUSE_NATIVE_COLD_MS");
    check_budget("native warm avg", warm_native_avg, "FUSE_NATIVE_WARM_MS");
}
