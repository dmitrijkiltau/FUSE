use std::path::PathBuf;
use std::process::Command;

fn example_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path
}

#[test]
fn runs_cli_hello_vm() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("vm")
        .arg(example_path("cli_hello.fuse"))
        .env("APP_GREETING", "Hi")
        .env("APP_DEFAULT_NAME", "Codex")
        .env("GREETING", "ShouldNotUse")
        .output()
        .expect("failed to run fusec");

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Hi, Codex!");
}

#[test]
fn runs_enum_match_vm() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("vm")
        .arg(example_path("enum_match.fuse"))
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["red", "rgb 1,2,3"]);
}

#[test]
fn runs_project_demo_ast() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(example_path("project_demo.fuse"))
        .env("APP_GREETING", "Hey")
        .env("APP_WHO", "Codex")
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["Hey, Codex!", "rgb 1,2,3"]);
}

#[test]
fn runs_interp_demo_ast() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(example_path("interp_demo.fuse"))
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["Hello, world!", "sum 3"]);
}

#[test]
fn runs_interp_demo_vm() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("vm")
        .arg(example_path("interp_demo.fuse"))
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["Hello, world!", "sum 3"]);
}
