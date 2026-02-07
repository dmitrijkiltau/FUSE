use std::process::Command;

fn example_path(name: &str) -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path.to_string_lossy().to_string()
}

fn run_example(backend: &str, example: &str) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(example_path(example))
        .output()
        .expect("failed to run fusec")
}

#[test]
fn native_task_api_smoke() {
    let output = run_example("native", "task_api.fuse");
    assert!(
        !output.status.success(),
        "expected native failure for spawn/await, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("native backend unsupported"),
        "unexpected native stderr: {stderr}"
    );
    assert!(
        stderr.contains("spawn/await"),
        "expected spawn/await in stderr: {stderr}"
    );
}
