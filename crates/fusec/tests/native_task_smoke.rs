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
        output.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "unexpected stdout: {stdout}");
    assert!(lines[0].starts_with("task-"), "unexpected task id: {}", lines[0]);
    assert_eq!(lines[1], "true", "unexpected done value: {}", lines[1]);
    assert_eq!(lines[2], "false", "unexpected cancel value: {}", lines[2]);
    assert_eq!(lines[3], "42", "unexpected task result: {}", lines[3]);
}
