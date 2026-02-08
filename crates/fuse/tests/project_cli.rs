use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_project_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("fuse_project_cli_test_{nanos}"));
    dir
}

#[test]
fn fmt_manifest_path_formats_project_module_graph() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");

    let main_src = "import { util } from \"./util\"\n\napp \"Demo\":\n  print( util( ) )\n";
    let util_src = "fn util( ) -> Int:\n  return  1\n";
    fs::write(dir.join("main.fuse"), main_src).expect("write main.fuse");
    fs::write(dir.join("util.fuse"), util_src).expect("write util.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("fmt")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse fmt");
    if !output.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    let got_main = fs::read_to_string(dir.join("main.fuse")).expect("read main.fuse");
    let got_util = fs::read_to_string(dir.join("util.fuse")).expect("read util.fuse");
    assert_eq!(got_main, fusec::format::format_source(main_src));
    assert_eq!(got_util, fusec::format::format_source(util_src));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_manifest_path_reports_cross_file_location() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");

    let main_src = r#"
import { broken } from "./util"

app "Demo":
  broken()
"#;
    let util_src = r#"
fn broken():
  let id: Missing = 1
"#;
    fs::write(dir.join("main.fuse"), main_src).expect("write main.fuse");
    fs::write(dir.join("util.fuse"), util_src).expect("write util.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse check");

    assert!(!output.status.success(), "check unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("util.fuse:3"), "stderr: {stderr}");
    assert!(stderr.contains("unknown type"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}
