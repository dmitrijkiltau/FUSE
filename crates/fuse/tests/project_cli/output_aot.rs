use super::*;

#[test]
fn check_color_always_emits_ansi_sequences() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_broken_project(&dir);

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("always")
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(contains_ansi(&stderr), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_color_never_emits_plain_text_only() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_broken_project(&dir);

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("never")
        .env("FUSE_COLOR_FORCE_TTY", "1")
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!contains_ansi(&stderr), "stderr: {stderr}");
    assert!(stderr.contains("error"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn no_color_disables_color_in_auto_mode() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_broken_project(&dir);

    let exe = env!("CARGO_BIN_EXE_fuse");
    let auto_color = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("auto")
        .env("FUSE_COLOR_FORCE_TTY", "1")
        .env_remove("NO_COLOR")
        .output()
        .expect("run fuse check auto");
    assert!(
        !auto_color.status.success(),
        "check unexpectedly succeeded (auto)"
    );
    let auto_stderr = String::from_utf8_lossy(&auto_color.stderr);
    assert!(contains_ansi(&auto_stderr), "stderr: {auto_stderr}");

    let no_color = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("auto")
        .env("FUSE_COLOR_FORCE_TTY", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("run fuse check no color");
    assert!(
        !no_color.status.success(),
        "check unexpectedly succeeded (no color)"
    );
    let no_color_stderr = String::from_utf8_lossy(&no_color.stderr);
    assert!(
        !contains_ansi(&no_color_stderr),
        "stderr: {no_color_stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_color_always_colorizes_runtime_log_lines() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_logging_project(&dir);

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("always")
        .output()
        .expect("run fuse run");
    assert!(
        output.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runtime-log"), "stderr: {stderr}");
    assert!(contains_ansi(&stderr), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn no_color_disables_runtime_log_color_in_auto_mode() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_logging_project(&dir);

    let exe = env!("CARGO_BIN_EXE_fuse");
    let auto_color = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("auto")
        .env("FUSE_COLOR_FORCE_TTY", "1")
        .env_remove("NO_COLOR")
        .output()
        .expect("run fuse run auto");
    assert!(
        auto_color.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&auto_color.stderr)
    );
    let auto_stderr = String::from_utf8_lossy(&auto_color.stderr);
    assert!(auto_stderr.contains("runtime-log"), "stderr: {auto_stderr}");
    assert!(contains_ansi(&auto_stderr), "stderr: {auto_stderr}");

    let no_color = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("auto")
        .env("FUSE_COLOR_FORCE_TTY", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("run fuse run no color");
    assert!(
        no_color.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&no_color.stderr)
    );
    let no_color_stderr = String::from_utf8_lossy(&no_color.stderr);
    assert!(
        no_color_stderr.contains("runtime-log"),
        "stderr: {no_color_stderr}"
    );
    assert!(
        !contains_ansi(&no_color_stderr),
        "stderr: {no_color_stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_emits_consistent_step_headers() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_broken_project(&dir);

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[check] start"), "stderr: {stderr}");
    assert!(stderr.contains("[check] failed"), "stderr: {stderr}");
    assert!(stderr.contains("error:"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_emits_consistent_step_headers() {
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
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse build");
    assert!(
        output.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[build] start"), "stderr: {stderr}");
    assert!(stderr.contains("[build] ok"), "stderr: {stderr}");
    assert!(
        !stderr.contains("[build] aot ["),
        "non-aot build unexpectedly emitted aot progress: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_writes_default_binary_output() {
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
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse build --aot");
    assert!(
        output.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[build] start"), "stderr: {stderr}");
    assert!(stderr.contains("[build] ok"), "stderr: {stderr}");

    let aot_path = default_aot_binary_path(&dir);
    assert!(aot_path.exists(), "expected {}", aot_path.display());
    assert!(
        dir.join(".fuse")
            .join("build")
            .join("program.native")
            .exists(),
        "expected cached native artifact"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_emits_progress_indicator() {
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
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse build --aot");
    assert!(
        output.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[build] start"), "stderr: {stderr}");
    assert!(stderr.contains("[build] ok"), "stderr: {stderr}");

    let steps = [
        "[build] aot [1/6] compile program",
        "[build] aot [2/6] write cache artifacts",
        "[build] aot [3/6] emit native object",
        "[build] aot [4/6] write runner source",
        "[build] aot [5/6] build link dependencies",
        "[build] aot [6/6] link final binary",
    ];
    let mut last = 0usize;
    for step in steps {
        let pos = stderr
            .find(step)
            .unwrap_or_else(|| panic!("missing step {step}; stderr: {stderr}"));
        assert!(pos >= last, "out-of-order step {step}; stderr: {stderr}");
        last = pos;
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_release_defaults_to_aot_output() {
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
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--release")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse build --release");
    assert!(
        output.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let aot_path = default_aot_binary_path(&dir);
    assert!(aot_path.exists(), "expected {}", aot_path.display());
    assert!(
        dir.join(".fuse")
            .join("build")
            .join("program.native")
            .exists(),
        "expected cached native artifact"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_without_release_remains_explicit_non_aot_path() {
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
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse build");
    assert!(
        output.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let aot_path = default_aot_binary_path(&dir);
    assert!(
        !aot_path.exists(),
        "did not expect AOT output for non-release build: {}",
        aot_path.display()
    );
    assert!(
        dir.join(".fuse")
            .join("build")
            .join("program.native")
            .exists(),
        "expected cached native artifact"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_accepts_release_with_aot_and_clean() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
app "Demo":
  print("ok")
"#,
    );

    let build_dir = dir.join(".fuse").join("build");
    fs::create_dir_all(&build_dir).expect("create build dir");
    fs::write(build_dir.join("stale.txt"), "stale").expect("write stale file");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .arg("--clean")
        .arg("--color")
        .arg("never")
        .output()
        .expect("run fuse build --aot --release --clean");
    assert!(
        output.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !build_dir.exists(),
        "expected clean build path to remove {}",
        build_dir.display()
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_build_info_env_prints_embedded_metadata() {
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
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    let info = Command::new(&aot)
        .env("FUSE_AOT_BUILD_INFO", "1")
        .output()
        .expect("run aot binary with build info env");
    assert!(
        info.status.success(),
        "info stderr: {}",
        String::from_utf8_lossy(&info.stderr)
    );
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(stdout.contains("mode=aot"), "stdout: {stdout}");
    assert!(stdout.contains("profile=release"), "stdout: {stdout}");
    assert!(stdout.contains("target="), "stdout: {stdout}");
    assert!(stdout.contains("rustc="), "stdout: {stdout}");
    assert!(stdout.contains("cli="), "stdout: {stdout}");
    assert!(stdout.contains("runtime_cache="), "stdout: {stdout}");
    assert!(stdout.contains("contract="), "stdout: {stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_rebuild_keeps_metadata_stable_for_identical_inputs() {
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
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run first fuse build --aot --release");
    assert!(
        build.status.success(),
        "first build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let first_meta = read_program_meta(&dir);
    let aot = default_aot_binary_path(&dir);
    let first_info = Command::new(&aot)
        .env("FUSE_AOT_BUILD_INFO", "1")
        .output()
        .expect("run first aot build info");
    assert!(
        first_info.status.success(),
        "first info stderr: {}",
        String::from_utf8_lossy(&first_info.stderr)
    );
    let first_info_line = String::from_utf8_lossy(&first_info.stdout)
        .trim()
        .to_string();

    fs::remove_dir_all(dir.join(".fuse").join("build")).expect("remove first build outputs");

    let rebuild = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run second fuse build --aot --release");
    assert!(
        rebuild.status.success(),
        "second build stderr: {}",
        String::from_utf8_lossy(&rebuild.stderr)
    );

    let second_meta = read_program_meta(&dir);
    let second_info = Command::new(&aot)
        .env("FUSE_AOT_BUILD_INFO", "1")
        .output()
        .expect("run second aot build info");
    assert!(
        second_info.status.success(),
        "second info stderr: {}",
        String::from_utf8_lossy(&second_info.stderr)
    );
    let second_info_line = String::from_utf8_lossy(&second_info.stdout)
        .trim()
        .to_string();

    assert_eq!(first_meta, second_meta);
    assert_eq!(first_info_line, second_info_line);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_build_info_short_circuits_startup_and_runtime() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
app "Demo":
  assert(false, "should-not-run")
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    let baseline = Command::new(&aot)
        .output()
        .expect("run aot baseline without build info");
    assert_eq!(
        baseline.status.code(),
        Some(1),
        "status: {:?}",
        baseline.status
    );
    let baseline_stderr = String::from_utf8_lossy(&baseline.stderr);
    assert!(
        baseline_stderr.contains("fatal: class=runtime_fatal"),
        "stderr: {baseline_stderr}"
    );

    let info = Command::new(&aot)
        .env("FUSE_AOT_BUILD_INFO", "1")
        .env("FUSE_AOT_STARTUP_TRACE", "1")
        .output()
        .expect("run aot binary with build-info short-circuit");
    assert!(
        info.status.success(),
        "info stderr: {}",
        String::from_utf8_lossy(&info.stderr)
    );
    let stdout = String::from_utf8_lossy(&info.stdout);
    let stderr = String::from_utf8_lossy(&info.stderr);
    assert!(stdout.contains("mode=aot"), "stdout: {stdout}");
    assert!(!stderr.contains("startup: pid="), "stderr: {stderr}");
    assert!(!stderr.contains("fatal:"), "stderr: {stderr}");
    assert!(!stderr.contains("should-not-run"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_runtime_respects_config_env_file_default_precedence() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
config App:
  greeting: String = "DefaultGreet"
  who: String = "DefaultWho"
  role: String = "DefaultRole"

app "Demo":
  print(App.greeting)
  print(App.who)
  print(App.role)
"#,
    )
    .expect("write main.fuse");
    fs::write(
        dir.join("config.toml"),
        r#"
[App]
greeting = "FileGreet"
who = "FileWho"
"#,
    )
    .expect("write config.toml");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    let run = Command::new(&aot)
        .current_dir(&dir)
        .env("FUSE_CONFIG", dir.join("config.toml"))
        .env("APP_GREETING", "EnvGreet")
        .output()
        .expect("run aot config precedence check");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["EnvGreet", "FileWho", "DefaultRole"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_runtime_supports_user_defined_config_env_overrides() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
type Profile:
  name: String
  level: Int = 1

enum Mode:
  Auto
  Manual(Int)

config App:
  profile: Profile = Profile(name="default")
  mode: Mode = Mode.Auto

app "Demo":
  print(App.profile.name)
  print(App.profile.level)
  match App.mode:
    Auto -> print("auto")
    Manual(v) -> print(v)
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    let run = Command::new(&aot)
        .current_dir(&dir)
        .env("APP_PROFILE", r#"{"name":"EnvUser"}"#)
        .env("APP_MODE", r#"{"type":"Manual","data":7}"#)
        .output()
        .expect("run aot config structured env overrides");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["EnvUser", "1", "7"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_runtime_emits_config_env_name_hint_for_mismatch() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
config App:
  dbUrl: String = "sqlite::memory:"

app "Demo":
  print(App.dbUrl)
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    let run = Command::new(&aot)
        .current_dir(&dir)
        .env("APP_DBURL", "sqlite://ignored.db")
        .output()
        .expect("run aot config mismatch hint check");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "sqlite::memory:"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("APP_DBURL") && stderr.contains("APP_DB_URL"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_runtime_error_uses_stable_fatal_envelope() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
app "Demo":
  assert(false, "boom")
"#,
    );

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    let run = Command::new(&aot).output().expect("run aot binary");
    assert_eq!(run.status.code(), Some(1), "status: {:?}", run.status);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("fatal: class=runtime_fatal"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("pid="), "stderr: {stderr}");
    assert!(stderr.contains("assert failed: boom"), "stderr: {stderr}");
    assert!(stderr.contains("mode=aot"), "stderr: {stderr}");
    assert!(stderr.contains("profile=release"), "stderr: {stderr}");
    assert!(stderr.contains("target="), "stderr: {stderr}");
    assert!(stderr.contains("contract="), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_runtime_panic_uses_exit_101_and_panic_fatal_envelope() {
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
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let runner = dir.join(".fuse").join("build").join("native_main.rs");
    let source = fs::read_to_string(&runner).expect("read native_main.rs");
    let patched = source.replacen(
        "fn run_program() -> Result<(), String> {\n",
        "fn run_program() -> Result<(), String> {\n    panic!(\"aot-panic-contract-test\");\n",
        1,
    );
    assert_ne!(
        source, patched,
        "failed to patch run_program in native_main.rs"
    );
    fs::write(&runner, patched).expect("write patched native_main.rs");
    relink_aot_runner_for_tests(&dir, true);

    let aot = default_aot_binary_path(&dir);
    let run = Command::new(&aot).output().expect("run patched aot binary");
    assert_eq!(run.status.code(), Some(101), "status: {:?}", run.status);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("fatal: class=panic"), "stderr: {stderr}");
    assert!(
        stderr.contains("panic_kind=panic_static_str aot-panic-contract-test"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("mode=aot"), "stderr: {stderr}");
    assert!(stderr.contains("profile=release"), "stderr: {stderr}");
    assert!(stderr.contains("target="), "stderr: {stderr}");
    assert!(stderr.contains("contract="), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_runner_wires_deterministic_panic_taxonomy() {
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
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let runner = dir.join(".fuse").join("build").join("native_main.rs");
    let source = fs::read_to_string(&runner).expect("read generated native_main.rs");
    assert!(
        source.contains("classify_panic_payload"),
        "runner source: {source}"
    );
    assert!(
        source.contains("format_panic_message"),
        "runner source: {source}"
    );
    assert!(
        source.contains("emit_fatal(\"panic\", &msg);"),
        "runner source: {source}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_startup_trace_env_emits_operability_header() {
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
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    let run = Command::new(&aot)
        .env("FUSE_AOT_STARTUP_TRACE", "1")
        .output()
        .expect("run aot binary with startup trace env");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    let startup_line = stderr
        .lines()
        .find(|line| line.starts_with("startup: "))
        .unwrap_or_else(|| panic!("missing startup line in stderr: {stderr}"));
    let expected_markers = [
        "startup: pid=",
        " mode=",
        " profile=",
        " target=",
        " rustc=",
        " cli=",
        " runtime_cache=",
        " contract=",
    ];
    let mut offsets = Vec::with_capacity(expected_markers.len());
    let mut search_from = 0usize;
    for marker in expected_markers {
        let rel = startup_line[search_from..]
            .find(marker)
            .unwrap_or_else(|| panic!("missing marker {marker} in startup line: {startup_line}"));
        let absolute = search_from + rel;
        offsets.push(absolute);
        search_from = absolute + marker.len();
    }
    for window in offsets.windows(2) {
        assert!(
            window[0] < window[1],
            "startup markers are out of order in {startup_line}"
        );
    }
    let pid_start = "startup: pid=".len();
    let mode_pos = offsets[1];
    let pid = &startup_line[pid_start..mode_pos];
    assert!(
        !pid.is_empty() && pid.chars().all(|ch| ch.is_ascii_digit()),
        "invalid pid segment in startup line: {startup_line}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_service_binary_handles_http_serve_routes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Docs"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
requires network

config App:
  port: String = env("PORT") ?? "3000"

service DocsApi at "/api":
  get "/health" -> Map<String, String>:
    return {"status": "ok"}

app "Docs":
  serve(App.port)
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let port = reserve_local_port();
    let aot = default_aot_binary_path(&dir);
    let child = Command::new(&aot)
        .env("PORT", port.to_string())
        .env("FUSE_HOST", "127.0.0.1")
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aot service");

    let response = match http_get_with_retry(port, "/api/health", 60) {
        Some(response) => response,
        None => {
            let output = child
                .wait_with_output()
                .expect("wait aot child after timeout");
            panic!(
                "failed to query aot health endpoint; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    };
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    assert!(
        response.contains("\"status\":\"ok\""),
        "response: {response}"
    );

    let output = child.wait_with_output().expect("wait aot child");
    assert!(
        output.status.success(),
        "aot stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_service_request_id_and_structured_logs_are_consistent() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Docs"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
requires network

config App:
  port: String = env("PORT") ?? "3000"

service DocsApi at "/api":
  get "/id" -> Map<String, String>:
    let req_id = request.header("x-request-id") ?? "missing"
    return {"id": req_id}

app "Docs":
  serve(App.port)
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let port = reserve_local_port();
    let aot = default_aot_binary_path(&dir);
    let child = Command::new(&aot)
        .env("PORT", port.to_string())
        .env("FUSE_HOST", "127.0.0.1")
        .env("FUSE_MAX_REQUESTS", "1")
        .env("FUSE_REQUEST_LOG", "structured")
        .env("FUSE_METRICS_HOOK", "stderr")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aot service");

    let request = format!(
        "GET /api/id HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Request-Id: aot-obs-1\r\nConnection: close\r\n\r\n"
    );
    let response = match http_request_with_retry(port, &request, 60) {
        Some(response) => response,
        None => {
            let output = child
                .wait_with_output()
                .expect("wait aot child after timeout");
            panic!(
                "failed to query aot id endpoint; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    };
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    let response_lower = response.to_ascii_lowercase();
    assert!(
        response_lower.contains("x-request-id: aot-obs-1"),
        "response: {response}"
    );
    assert!(
        response.contains(r#"{"id":"aot-obs-1"}"#),
        "response: {response}"
    );

    let output = child.wait_with_output().expect("wait aot child");
    assert!(
        output.status.success(),
        "aot stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"event\":\"http.request\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"runtime\":\"native\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"request_id\":\"aot-obs-1\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"metric\":\"http.server.request\""),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_aot_release_can_optionally_default_to_structured_request_logs() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Docs"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
requires network

config App:
  port: String = env("PORT") ?? "3000"

service DocsApi at "/api":
  get "/id" -> Map<String, String>:
    let req_id = request.header("x-request-id") ?? "missing"
    return {"id": req_id}

app "Docs":
  serve(App.port)
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let port = reserve_local_port();
    let aot = default_aot_binary_path(&dir);
    let child = Command::new(&aot)
        .env("PORT", port.to_string())
        .env("FUSE_HOST", "127.0.0.1")
        .env("FUSE_MAX_REQUESTS", "1")
        .env("FUSE_AOT_REQUEST_LOG_DEFAULT", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aot service");

    let request = format!(
        "GET /api/id HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Request-Id: aot-default-log\r\nConnection: close\r\n\r\n"
    );
    let response = match http_request_with_retry(port, &request, 60) {
        Some(response) => response,
        None => {
            let output = child
                .wait_with_output()
                .expect("wait aot child after timeout");
            panic!(
                "failed to query aot id endpoint; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    };
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    assert!(
        response.contains(r#"{"id":"aot-default-log"}"#),
        "response: {response}"
    );

    let output = child.wait_with_output().expect("wait aot child");
    assert!(
        output.status.success(),
        "aot stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"event\":\"http.request\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"request_id\":\"aot-default-log\""),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn build_aot_signal_termination_is_graceful_and_deterministic() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Docs"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
requires network

config App:
  port: String = env("PORT") ?? "3000"

service DocsApi at "/api":
  get "/health" -> Map<String, String>:
    return {"status": "ok"}

app "Docs":
  serve(App.port)
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--aot")
        .arg("--release")
        .output()
        .expect("run fuse build --aot --release");
    assert!(
        build.status.success(),
        "build stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let aot = default_aot_binary_path(&dir);
    for (signal, expected_name) in [("-TERM", "SIGTERM"), ("-INT", "SIGINT")] {
        let port = reserve_local_port();
        let mut child = Command::new(&aot)
            .env("PORT", port.to_string())
            .env("FUSE_HOST", "127.0.0.1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn aot service");
        let mut stderr_pipe = child.stderr.take().expect("child stderr pipe");

        let response = match http_get_with_retry(port, "/api/health", 60) {
            Some(response) => response,
            None => {
                let _ = child.kill();
                let _ = wait_for_child_exit_status(&mut child, Duration::from_secs(2));
                let mut stderr = String::new();
                let _ = stderr_pipe.read_to_string(&mut stderr);
                panic!("failed to query aot health endpoint; stderr: {stderr}");
            }
        };
        assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");

        send_unix_signal(child.id(), signal);
        let status = wait_for_child_exit_status(&mut child, Duration::from_secs(5));
        assert_eq!(status.code(), Some(0), "status: {status:?}");

        let mut stderr = String::new();
        stderr_pipe
            .read_to_string(&mut stderr)
            .expect("read child stderr");
        assert!(!stderr.contains("fatal:"), "stderr: {stderr}");
        assert!(
            stderr.contains(&format!("shutdown: runtime=native signal={expected_name}")),
            "stderr: {stderr}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}
