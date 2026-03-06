use super::*;

#[test]
fn clean_cache_removes_nested_fuse_cache_directories_only() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let root_cache = dir.join(".fuse-cache");
    let dep_cache = dir.join("deps").join("helper").join(".fuse-cache");
    let build_dir = dir.join(".fuse").join("build");

    fs::create_dir_all(&root_cache).expect("create root cache");
    fs::create_dir_all(&dep_cache).expect("create dep cache");
    fs::create_dir_all(&build_dir).expect("create build dir");
    fs::write(root_cache.join("check-123.tsv"), "cached").expect("write root cache file");
    fs::write(dep_cache.join("lsp-index-123.json"), "{}").expect("write dep cache file");
    fs::write(build_dir.join("program.meta"), "keep").expect("write build cache file");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("clean")
        .arg("--cache")
        .arg(&dir)
        .output()
        .expect("run fuse clean --cache");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root_cache.exists(), "root .fuse-cache was not removed");
    assert!(!dep_cache.exists(), "nested .fuse-cache was not removed");
    assert!(build_dir.exists(), ".fuse/build should be preserved");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[clean] removed 2 .fuse-cache directories under"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("[clean] ok"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn clean_cache_defaults_to_current_directory() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let cache_dir = dir.join(".fuse-cache");
    fs::create_dir_all(&cache_dir).expect("create cache dir");
    fs::write(cache_dir.join("check-123.tsv"), "cached").expect("write cache file");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .current_dir(&dir)
        .arg("clean")
        .arg("--cache")
        .output()
        .expect("run fuse clean --cache from cwd");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!cache_dir.exists(), "cwd .fuse-cache was not removed");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn clean_cache_accepts_manifest_path() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    write_basic_manifest_project(
        &dir,
        r#"
app "Demo":
  print("ok")
"#,
    );

    let cache_dir = dir.join(".fuse-cache");
    fs::create_dir_all(&cache_dir).expect("create cache dir");
    fs::write(cache_dir.join("lsp-index-123.json"), "{}").expect("write cache file");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("clean")
        .arg("--cache")
        .arg("--manifest-path")
        .arg(dir.join("fuse.toml"))
        .output()
        .expect("run fuse clean --cache --manifest-path");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !cache_dir.exists(),
        "manifest root .fuse-cache was not removed"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn clean_cache_requires_cache_flag() {
    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("clean")
        .output()
        .expect("run fuse clean");

    assert!(!output.status.success(), "clean unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fuse clean requires --cache"),
        "stderr: {stderr}"
    );
}
