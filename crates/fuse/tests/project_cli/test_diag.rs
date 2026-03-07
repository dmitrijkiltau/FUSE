use super::*;

#[test]
fn test_emits_consistent_step_headers() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
app "Demo":
  print("ok")

test "smoke":
  assert(1 == 1)
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("test")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse test");
    assert!(
        output.status.success(),
        "test stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[test] start"), "stderr: {stderr}");
    assert!(stderr.contains("[test] ok"), "stderr: {stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok smoke"), "stdout: {stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_filter_runs_matching_tests_only() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
app "Demo":
  print("ok")

test "smoke-fast":
  assert(1 == 1)

test "slow-fail":
  assert(1 == 2)
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("test")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--filter")
        .arg("smoke")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse test with filter");
    assert!(
        output.status.success(),
        "test stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[test] start"), "stderr: {stderr}");
    assert!(stderr.contains("[test] ok"), "stderr: {stderr}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok smoke-fast"), "stdout: {stdout}");
    assert!(!stdout.contains("slow-fail"), "stdout: {stdout}");
    assert!(stdout.contains("ok (1 tests)"), "stdout: {stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn filter_option_is_rejected_for_non_test_commands() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
app "Demo":
  print("ok")
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--filter")
        .arg("smoke")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse run with --filter");
    assert!(!output.status.success(), "run unexpectedly succeeded");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: --filter is only supported for fuse test"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_validation_errors_use_exit_code_2_and_consistent_step_footer() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
fn main(name: String):
  print("hello " + name)

app "Demo":
  main("ok")
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("never")
        .arg("--")
        .arg("--unknown=1")
        .output()
        .expect("run fuse run");
    assert_eq!(output.status.code(), Some(2), "status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[run] start"), "stderr: {stderr}");
    assert!(
        stderr.contains("[run] validation failed"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("validation failed"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unknown_command_uses_error_prefix() {
    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("bad-command")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse bad-command");
    assert!(!output.status.success(), "command unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: unknown command"),
        "stderr: {stderr}"
    );
}

#[test]
fn check_strict_architecture_flag_enforces_additional_sema_guards() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
requires db

fn main() -> Int:
  return 1

app "Demo":
  print(main())
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--strict-architecture")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse check --strict-architecture");
    assert!(!output.status.success(), "strict check unexpectedly passed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("strict architecture: capability purity violation"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_diagnostics_json_emits_structured_output_for_project_mode() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
import { broken } from "./util"

app "Demo":
  broken()
"#,
    )
    .expect("write main.fuse");
    fs::write(
        dir.join("util.fuse"),
        r#"
fn broken():
  let value: Missing = 1
"#,
    )
    .expect("write util.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--diagnostics")
        .arg("json")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse check --diagnostics json");
    assert!(!output.status.success(), "check unexpectedly succeeded");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"kind\":\"command_step\"") && stderr.contains("\"command\":\"check\""),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("\"message\":\"start\""), "stderr: {stderr}");
    assert!(
        stderr.contains("\"message\":\"failed\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"kind\":\"diagnostic\""),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("\"level\":\"error\""), "stderr: {stderr}");
    assert!(stderr.contains("util.fuse"), "stderr: {stderr}");
    assert!(!stderr.contains("error:"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_diagnostics_json_emits_structured_output_for_delegated_mode() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let entry = dir.join("main.fuse");
    fs::write(
        &entry,
        r#"
app "Demo":
  let value: Missing = 1
  print(value)
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--diagnostics")
        .arg("json")
        .arg("--color")
        .arg("never")
        .arg(&entry)
        .output()
        .expect("run delegated fuse check --diagnostics json");
    assert!(!output.status.success(), "check unexpectedly succeeded");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"kind\":\"command_step\"") && stderr.contains("\"command\":\"check\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"kind\":\"diagnostic\""),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("\"level\":\"error\""), "stderr: {stderr}");
    assert!(stderr.contains("main.fuse"), "stderr: {stderr}");
    assert!(!stderr.contains("error:"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_validation_errors_emit_json_step_events_when_diagnostics_json_enabled() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
fn main(name: String):
  print("hello " + name)

app "Demo":
  main("ok")
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--diagnostics")
        .arg("json")
        .arg("--color")
        .arg("never")
        .arg("--")
        .arg("--unknown=1")
        .output()
        .expect("run fuse run --diagnostics json");
    assert_eq!(output.status.code(), Some(2), "status: {:?}", output.status);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"kind\":\"command_step\"") && stderr.contains("\"command\":\"run\""),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("\"message\":\"start\""), "stderr: {stderr}");
    assert!(
        stderr.contains("\"message\":\"validation failed\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"code\":\"unknown_flag\""),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("[run]"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diagnostics_json_includes_html_attr_codes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let entry = dir.join("main.fuse");
    fs::write(
        &entry,
        r#"
fn page(css: String) -> Html:
  let one = link({"rel": "stylesheet", "href": css})
  let two = div(class="hero", id="main")
  return two

app "Demo":
  print(html.render(page("/assets/main.css")))
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--diagnostics")
        .arg("json")
        .arg("--color")
        .arg("never")
        .arg(&entry)
        .output()
        .expect("run fuse check --diagnostics json");
    assert!(!output.status.success(), "check unexpectedly succeeded");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"code\":\"FUSE_HTML_ATTR_MAP\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"code\":\"FUSE_HTML_ATTR_COMMA\""),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diagnostics_json_includes_typed_query_codes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let entry = dir.join("main.fuse");
    fs::write(
        &entry,
        r#"
requires db

type User:
  id: Int
  name: String

fn load_missing_select() -> List<User>:
  return db.from("users").all<User>()

fn load_field_mismatch() -> List<User>:
  return db.from("users").select(["id"]).all<User>()
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--diagnostics")
        .arg("json")
        .arg("--color")
        .arg("never")
        .arg(&entry)
        .output()
        .expect("run fuse check --diagnostics json");
    assert!(!output.status.success(), "check unexpectedly succeeded");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"code\":\"FUSE_TYPED_QUERY_SELECT\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"code\":\"FUSE_TYPED_QUERY_FIELD_MISMATCH\""),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}
