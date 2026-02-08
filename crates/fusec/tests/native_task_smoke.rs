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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected native task success, stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.len() >= 4, "expected 4 output lines, got: {stdout}");
    assert!(
        lines[0].starts_with("task-"),
        "expected task id on line 1, got: {}",
        lines[0]
    );
    assert_eq!(lines[1], "true");
    assert_eq!(lines[2], "false");
    assert_eq!(lines[3], "42");
}
