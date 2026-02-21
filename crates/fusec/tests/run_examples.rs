use std::path::PathBuf;
use std::process::Command;
use std::{io::Write, process::Stdio};

fn example_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path
}

fn run_example_with_stdin(backend: &str, example: &str, stdin_text: &str) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(example_path(example))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run fusec");
    {
        let mut stdin = child.stdin.take().expect("missing child stdin");
        stdin
            .write_all(stdin_text.as_bytes())
            .expect("failed to write stdin");
    }
    child.wait_with_output().expect("failed to wait for fusec")
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

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Hi, Codex!");
}

#[test]
fn runs_cli_hello_native() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("native")
        .arg(example_path("cli_hello.fuse"))
        .env("APP_GREETING", "Hi")
        .env("APP_DEFAULT_NAME", "Codex")
        .env("GREETING", "ShouldNotUse")
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Hi, Codex!");
}

#[test]
fn runs_cli_args_native() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("native")
        .arg(example_path("cli_args.fuse"))
        .output()
        .expect("failed to run fusec");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["Hello, world"]);
}

#[test]
fn runs_cli_input_vm() {
    let output = run_example_with_stdin("vm", "cli_input.fuse", "Codex\n");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Name: Hello, Codex\n"
    );
}

#[test]
fn runs_cli_input_native() {
    let output = run_example_with_stdin("native", "cli_input.fuse", "Codex\n");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Name: Hello, Codex\n"
    );
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

#[test]
fn runs_interp_demo_native() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("native")
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
