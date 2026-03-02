use super::*;

#[test]
fn check_reports_transitive_dependency_conflicts_with_origin_paths() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
AuthA = { path = "./deps/auth-a" }
AuthB = { path = "./deps/auth-b" }
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    fs::create_dir_all(dir.join("deps").join("auth-a")).expect("create auth-a");
    fs::write(
        dir.join("deps").join("auth-a").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "AuthA"

[dependencies]
Shared = { path = "../shared-one" }
"#,
    )
    .expect("write auth-a manifest");
    fs::write(
        dir.join("deps").join("auth-a").join("lib.fuse"),
        r#"
fn local() -> Int:
  return 1
"#,
    )
    .expect("write auth-a lib");

    fs::create_dir_all(dir.join("deps").join("auth-b")).expect("create auth-b");
    fs::write(
        dir.join("deps").join("auth-b").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "AuthB"

[dependencies]
Shared = { path = "../shared-two" }
"#,
    )
    .expect("write auth-b manifest");
    fs::write(
        dir.join("deps").join("auth-b").join("lib.fuse"),
        r#"
fn local() -> Int:
  return 2
"#,
    )
    .expect("write auth-b lib");

    fs::create_dir_all(dir.join("deps").join("shared-one")).expect("create shared-one");
    fs::write(
        dir.join("deps").join("shared-one").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "SharedOne"
"#,
    )
    .expect("write shared-one manifest");
    fs::write(
        dir.join("deps").join("shared-one").join("lib.fuse"),
        r#"
fn value() -> Int:
  return 10
"#,
    )
    .expect("write shared-one lib");

    fs::create_dir_all(dir.join("deps").join("shared-two")).expect("create shared-two");
    fs::write(
        dir.join("deps").join("shared-two").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "SharedTwo"
"#,
    )
    .expect("write shared-two manifest");
    fs::write(
        dir.join("deps").join("shared-two").join("lib.fuse"),
        r#"
fn value() -> Int:
  return 20
"#,
    )
    .expect("write shared-two lib");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_CONFLICTING_SPECS]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Shared requested with conflicting specs"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("deps/auth-a/fuse.toml"), "stderr: {stderr}");
    assert!(stderr.contains("deps/auth-b/fuse.toml"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_rejects_dependency_with_multiple_git_reference_fields() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Bad = { git = "https://example.com/demo.git", tag = "v1.0.0", branch = "main" }
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_GIT_REF_CONFLICT]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Bad must specify at most one of rev, tag, branch, version"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_invalid_dependency_source_hint() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Bad = "1.2.3"
"#,
    )
    .expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_INVALID_SOURCE]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Bad has invalid source \"1.2.3\""),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("use a relative/absolute path or { git = \"...\" }"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_rejects_dependency_missing_source_required_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_minimal_check_project(
        &dir,
        r#"[dependencies]
Bad = {}
"#,
    );

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_SOURCE_REQUIRED]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Bad must specify either path or git"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_rejects_dependency_refs_without_git_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_minimal_check_project(
        &dir,
        r#"[dependencies]
Bad = { tag = "v1.2.3" }
"#,
    );

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_GIT_REQUIRED_FOR_REFS]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Bad must specify git when using rev/tag/branch/version/subdir"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_rejects_dependency_empty_path_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_minimal_check_project(
        &dir,
        r#"[dependencies]
Bad = { path = "   " }
"#,
    );

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[FUSE_DEP_PATH_EMPTY]"), "stderr: {stderr}");
    assert!(
        stderr.contains("dependency Bad path cannot be empty"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_rejects_dependency_empty_subdir_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_minimal_check_project(
        &dir,
        r#"[dependencies]
Bad = { git = "https://example.com/demo.git", subdir = "   " }
"#,
    );

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_SUBDIR_EMPTY]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Bad subdir cannot be empty"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_rejects_path_dependency_with_git_fields_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_minimal_check_project(
        &dir,
        r#"[dependencies]
Bad = { path = "./deps/bad", branch = "main" }
"#,
    );

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_PATH_FIELDS_INVALID]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr
            .contains("path dependencies cannot include git/rev/tag/branch/version/subdir fields"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_dependency_path_not_found_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_minimal_check_project(
        &dir,
        r#"[dependencies]
Bad = { path = "./deps/missing" }
"#,
    );

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_PATH_NOT_FOUND]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Bad path does not exist"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("fix the dependency path in fuse.toml"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_dependency_git_subdir_not_found_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let git_src = dir.join("git_src");
    fs::create_dir_all(&git_src).expect("create git source");
    fs::write(
        git_src.join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "GitDep"
"#,
    )
    .expect("write git source manifest");
    fs::write(git_src.join("lib.fuse"), "fn value() -> Int:\n  return 1\n")
        .expect("write git source lib");
    let init = Command::new("git")
        .arg("init")
        .arg(&git_src)
        .output()
        .expect("git init");
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = Command::new("git")
        .arg("-C")
        .arg(&git_src)
        .arg("add")
        .arg(".")
        .output()
        .expect("git add");
    assert!(
        add.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let commit = Command::new("git")
        .arg("-C")
        .arg(&git_src)
        .arg("-c")
        .arg("user.name=Fuse Test")
        .arg("-c")
        .arg("user.email=fuse@example.test")
        .arg("commit")
        .arg("-m")
        .arg("init")
        .output()
        .expect("git commit");
    assert!(
        commit.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    write_minimal_check_project(
        &dir,
        &format!(
            r#"[dependencies]
Bad = {{ git = "file://{}", subdir = "missing-subdir" }}
"#,
            git_src.display()
        ),
    );

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_DEP_SUBDIR_NOT_FOUND]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("dependency Bad subdir does not exist"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("fix the dependency subdir in fuse.toml"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_accepts_semantically_identical_dependency_specs_without_false_conflict() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
AuthA = { path = "./deps/auth-a" }
AuthB = { path = "./deps/auth-b" }
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    fs::create_dir_all(dir.join("deps").join("auth-a")).expect("create auth-a");
    fs::write(
        dir.join("deps").join("auth-a").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "AuthA"

[dependencies]
Shared = { path = "../shared" }
"#,
    )
    .expect("write auth-a manifest");
    fs::write(
        dir.join("deps").join("auth-a").join("lib.fuse"),
        "fn value() -> Int:\n  return 1\n",
    )
    .expect("write auth-a lib");

    fs::create_dir_all(dir.join("deps").join("auth-b")).expect("create auth-b");
    fs::write(
        dir.join("deps").join("auth-b").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "AuthB"

[dependencies]
Shared = { path = "..\\shared" }
"#,
    )
    .expect("write auth-b manifest");
    fs::write(
        dir.join("deps").join("auth-b").join("lib.fuse"),
        "fn value() -> Int:\n  return 2\n",
    )
    .expect("write auth-b lib");

    fs::create_dir_all(dir.join("deps").join("shared")).expect("create shared");
    fs::write(
        dir.join("deps").join("shared").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "Shared"
"#,
    )
    .expect("write shared manifest");
    fs::write(
        dir.join("deps").join("shared").join("lib.fuse"),
        "fn shared() -> Int:\n  return 10\n",
    )
    .expect("write shared lib");

    let output = run_check_project(&dir);
    assert!(
        output.status.success(),
        "check stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_keeps_lockfile_stable_when_dependencies_are_unchanged() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Helper = { path = "./deps/helper" }
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");
    fs::create_dir_all(dir.join("deps").join("helper")).expect("create helper dep");
    fs::write(
        dir.join("deps").join("helper").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "Helper"
"#,
    )
    .expect("write helper manifest");
    fs::write(
        dir.join("deps").join("helper").join("lib.fuse"),
        r#"
fn prefix(name: String) -> String:
  return "dep-" + name
"#,
    )
    .expect("write helper lib");

    run_build_project(&dir);
    let lock_path = dir.join("fuse.lock");
    let first = fs::read_to_string(&lock_path).expect("read lock after first build");
    run_build_project(&dir);
    let second = fs::read_to_string(&lock_path).expect("read lock after second build");

    assert_eq!(first, second, "lockfile should remain stable");
    assert!(
        second.contains("[dependencies.Helper]"),
        "lockfile: {second}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_supports_dependency_manifest_variants() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
AuthString = "./deps/auth-string"
AuthInline = { path = "./deps/auth-inline" }

[dependencies.AuthTable]
path = "./deps/auth-table"
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
import AuthString from "dep:AuthString/lib"
import AuthInline from "dep:AuthInline/lib"
import AuthTable from "dep:AuthTable/lib"

app "Demo":
  let a = AuthString.plus_one(1)
  let b = AuthInline.plus_one(a)
  let c = AuthTable.plus_one(b)
  print(c)
"#,
    )
    .expect("write main.fuse");

    for dep in ["auth-string", "auth-inline", "auth-table"] {
        let dep_dir = dir.join("deps").join(dep);
        fs::create_dir_all(&dep_dir).expect("create dep dir");
        fs::write(
            dep_dir.join("fuse.toml"),
            r#"
[package]
entry = "lib.fuse"
app = "Dep"
"#,
        )
        .expect("write dep manifest");
        fs::write(
            dep_dir.join("lib.fuse"),
            r#"
fn plus_one(value: Int) -> Int:
  return value + 1
"#,
        )
        .expect("write dep lib");
    }

    let exe = env!("CARGO_BIN_EXE_fuse");
    let check = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");
    assert!(
        check.status.success(),
        "check stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let run = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse run");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "4");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_supports_windows_style_dependency_path_separators() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Helper = { path = './deps\helper' }
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
import Helper from "dep:Helper/lib"

app "Demo":
  print(Helper.prefix("ok"))
"#,
    )
    .expect("write main.fuse");

    let helper_dir = dir.join("deps").join("helper");
    fs::create_dir_all(&helper_dir).expect("create helper dep");
    fs::write(
        helper_dir.join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "Helper"
"#,
    )
    .expect("write helper manifest");
    fs::write(
        helper_dir.join("lib.fuse"),
        r#"
fn prefix(name: String) -> String:
  return "dep-" + name
"#,
    )
    .expect("write helper lib");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let check = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");
    assert!(
        check.status.success(),
        "check stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let run = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse run");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "dep-ok");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_lockfile_version_error_code_with_remediation_hint() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Helper = { path = "./deps/helper" }
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    let helper_dir = dir.join("deps").join("helper");
    fs::create_dir_all(&helper_dir).expect("create helper dep");
    fs::write(
        helper_dir.join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "Helper"
"#,
    )
    .expect("write helper manifest");
    fs::write(
        helper_dir.join("lib.fuse"),
        "fn helper() -> Int:\n  return 1\n",
    )
    .expect("write helper lib");

    fs::write(
        dir.join("fuse.lock"),
        r#"
version = 99
"#,
    )
    .expect("write fuse.lock");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_UNSUPPORTED_VERSION]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("delete fuse.lock and rerun 'fuse build'"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_stale_lock_path_with_remediation_hint() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Helper = { path = "./deps/helper" }
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    let helper_dir = dir.join("deps").join("helper");
    fs::create_dir_all(&helper_dir).expect("create helper dep");
    fs::write(
        helper_dir.join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "Helper"
"#,
    )
    .expect("write helper manifest");
    fs::write(
        helper_dir.join("lib.fuse"),
        "fn helper() -> Int:\n  return 1\n",
    )
    .expect("write helper lib");

    let requested = format!(
        "path:{}",
        fs::canonicalize(dir.join("deps").join("helper"))
            .expect("canonicalize helper path")
            .display()
    );
    fs::write(
        dir.join("fuse.lock"),
        format!(
            r#"
version = 1

[dependencies.Helper]
source = "path"
path = "./deps/missing"
requested = "{requested}"
"#
        ),
    )
    .expect("write stale fuse.lock");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_ENTRY_PATH_NOT_FOUND]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("delete fuse.lock and rerun 'fuse build'"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_lock_entry_missing_path_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let requested = write_single_helper_dep_project(&dir);

    fs::write(
        dir.join("fuse.lock"),
        format!(
            r#"
version = 1

[dependencies.Helper]
source = "path"
requested = "{requested}"
"#
        ),
    )
    .expect("write fuse.lock");

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_ENTRY_MISSING_PATH]"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_lock_entry_unknown_source_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let requested = write_single_helper_dep_project(&dir);

    fs::write(
        dir.join("fuse.lock"),
        format!(
            r#"
version = 1

[dependencies.Helper]
source = "archive"
requested = "{requested}"
"#
        ),
    )
    .expect("write fuse.lock");

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_ENTRY_UNKNOWN_SOURCE]"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("unknown lock source"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_lock_parse_failure_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_single_helper_dep_project(&dir);

    fs::write(
        dir.join("fuse.lock"),
        r#"
version = 1

[dependencies.Helper
source = "path"
"#,
    )
    .expect("write invalid fuse.lock");

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_PARSE_FAILED]"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_lock_entry_missing_rev_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let (git_url, _rev) = create_local_git_dep_repo(&dir);
    write_minimal_check_project(
        &dir,
        &format!(
            r#"[dependencies]
Helper = {{ git = "{git_url}" }}
"#
        ),
    );
    run_build_project(&dir);
    let lock_text = fs::read_to_string(dir.join("fuse.lock")).expect("read fuse.lock");
    let requested = extract_lock_string_field(&lock_text, "requested");
    let git = extract_lock_string_field(&lock_text, "git");

    fs::write(
        dir.join("fuse.lock"),
        format!(
            r#"
version = 1

[dependencies.Helper]
source = "git"
git = "{git}"
requested = "{requested}"
"#
        ),
    )
    .expect("write fuse.lock");

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_ENTRY_MISSING_REV]"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_lock_entry_missing_git_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let (git_url, _rev) = create_local_git_dep_repo(&dir);
    write_minimal_check_project(
        &dir,
        &format!(
            r#"[dependencies]
Helper = {{ git = "{git_url}" }}
"#
        ),
    );
    run_build_project(&dir);
    let lock_text = fs::read_to_string(dir.join("fuse.lock")).expect("read fuse.lock");
    let requested = extract_lock_string_field(&lock_text, "requested");
    let rev = extract_lock_string_field(&lock_text, "rev");

    fs::write(
        dir.join("fuse.lock"),
        format!(
            r#"
version = 1

[dependencies.Helper]
source = "git"
rev = "{rev}"
requested = "{requested}"
"#
        ),
    )
    .expect("write fuse.lock");

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_ENTRY_MISSING_GIT]"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_reports_lock_entry_subdir_not_found_code() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    let (git_url, _rev) = create_local_git_dep_repo(&dir);
    write_minimal_check_project(
        &dir,
        &format!(
            r#"[dependencies]
Helper = {{ git = "{git_url}" }}
"#
        ),
    );
    run_build_project(&dir);
    let lock_text = fs::read_to_string(dir.join("fuse.lock")).expect("read fuse.lock");
    let requested = extract_lock_string_field(&lock_text, "requested");
    let rev = extract_lock_string_field(&lock_text, "rev");
    let git = extract_lock_string_field(&lock_text, "git");

    fs::write(
        dir.join("fuse.lock"),
        format!(
            r#"
version = 1

[dependencies.Helper]
source = "git"
git = "{git}"
rev = "{rev}"
subdir = "missing-subdir"
requested = "{requested}"
"#
        ),
    )
    .expect("write fuse.lock");

    let output = run_check_project(&dir);
    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[FUSE_LOCK_ENTRY_SUBDIR_NOT_FOUND]"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_lockfile_ignores_non_spec_manifest_edits() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_single_helper_dep_project(&dir);
    run_build_project(&dir);

    let lock_path = dir.join("fuse.lock");
    let first = fs::read_to_string(&lock_path).expect("read initial lockfile");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"
# formatting-only edit

[dependencies]
Helper = { path = "./deps/helper" }
"#,
    )
    .expect("rewrite fuse.toml");

    run_build_project(&dir);
    let second = fs::read_to_string(&lock_path).expect("read rewritten lockfile");
    assert_eq!(first, second, "lockfile should ignore non-spec edits");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_lockfile_refreshes_when_requested_spec_fingerprint_changes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_single_helper_dep_project(&dir);
    run_build_project(&dir);

    let lock_path = dir.join("fuse.lock");
    let first = fs::read_to_string(&lock_path).expect("read initial lockfile");

    let helper_two = dir.join("deps").join("helper-two");
    fs::create_dir_all(&helper_two).expect("create helper-two dep");
    fs::write(
        helper_two.join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "HelperTwo"
"#,
    )
    .expect("write helper-two manifest");
    fs::write(
        helper_two.join("lib.fuse"),
        "fn helper() -> Int:\n  return 2\n",
    )
    .expect("write helper-two lib");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Helper = { path = "./deps/helper-two" }
"#,
    )
    .expect("rewrite fuse.toml with updated dependency source");

    run_build_project(&dir);
    let second = fs::read_to_string(&lock_path).expect("read refreshed lockfile");
    assert_ne!(
        first, second,
        "lockfile should refresh when requested spec fingerprint changes"
    );
    assert!(second.contains("helper-two"), "lockfile: {second}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_invalidates_cached_ir_when_dependency_source_changes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"

[dependencies]
Helper = { path = "./deps/helper" }
"#,
    )
    .expect("write root fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
import Helper from "dep:Helper/lib"

fn main(name: String = "world"):
  print(Helper.prefix(name))

app "Demo":
  main()
"#,
    )
    .expect("write main.fuse");
    fs::create_dir_all(dir.join("deps").join("helper")).expect("create helper dep");
    fs::write(
        dir.join("deps").join("helper").join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "Helper"
"#,
    )
    .expect("write helper manifest");
    fs::write(
        dir.join("deps").join("helper").join("lib.fuse"),
        r#"
fn prefix(name: String) -> String:
  return "source-" + name
"#,
    )
    .expect("write helper lib");

    run_build_project(&dir);

    let cached_program = r#"
fn main(name: String = "world"):
  print("cache-" + name)

app "Demo":
  main()
"#;
    overwrite_cached_ir_from_source(&dir, cached_program);

    fs::write(
        dir.join("deps").join("helper").join("lib.fuse"),
        r#"
fn prefix(name: String) -> String:
  return "dep-" + name
"#,
    )
    .expect("rewrite helper lib");

    let run = run_with_named_arg(&dir, "--name=changed");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "dep-changed");

    let _ = fs::remove_dir_all(&dir);
}
