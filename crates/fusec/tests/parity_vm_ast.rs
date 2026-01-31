use std::process::Command;

use fuse_rt::json;

fn example_path(name: &str) -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path.to_string_lossy().to_string()
}

fn run_example(
    backend: &str,
    example: &str,
    envs: &[(&str, &str)],
) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(example_path(example));
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("failed to run fusec")
}

fn normalize_error(stderr: &str) -> String {
    let text = stderr.trim();
    let json_text = text.strip_prefix("run error: ").unwrap_or(text);
    let parsed = json::decode(json_text).expect("expected JSON error");
    json::encode(&parsed)
}

#[test]
fn parity_cli_hello() {
    let envs = [("GREETING", "Hi"), ("NAME", "Codex")];
    let ast = run_example("ast", "cli_hello.fuse", &envs);
    let vm = run_example("vm", "cli_hello.fuse", &envs);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_interp_demo() {
    let ast = run_example("ast", "interp_demo.fuse", &[]);
    let vm = run_example("vm", "interp_demo.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_enum_match() {
    let ast = run_example("ast", "enum_match.fuse", &[]);
    let vm = run_example("vm", "enum_match.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_project_demo_error() {
    let envs = [("DEMO_FAIL", "1")];
    let ast = run_example("ast", "project_demo.fuse", &envs);
    let vm = run_example("vm", "project_demo.fuse", &envs);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!vm.status.success(), "expected vm failure");

    let ast_err = normalize_error(&String::from_utf8_lossy(&ast.stderr));
    let vm_err = normalize_error(&String::from_utf8_lossy(&vm.stderr));
    assert_eq!(ast_err, vm_err);
}
