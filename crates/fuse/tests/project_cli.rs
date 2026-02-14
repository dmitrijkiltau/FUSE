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

#[test]
fn build_runs_external_sass_pipeline_when_assets_configured() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"

[assets]
scss = "assets/scss"
css = "public/css"
watch = true
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");

    let main_src = r#"
app "Demo":
  print("ok")
"#;
    fs::write(dir.join("main.fuse"), main_src).expect("write main.fuse");

    let scss_dir = dir.join("assets").join("scss");
    fs::create_dir_all(&scss_dir).expect("create scss dir");
    fs::write(
        scss_dir.join("app.scss"),
        "$c: #fff;\nbody { color: $c; }\n",
    )
    .expect("write scss");

    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let sass_path = bin_dir.join("sass");
    let sass_script = r#"#!/usr/bin/env bash
set -euo pipefail
mapping=""
for arg in "$@"; do
  case "$arg" in
    --*) ;;
    *) mapping="$arg" ;;
  esac
done
src="${mapping%%:*}"
dst="${mapping#*:}"
if [[ -d "$src" ]]; then
  mkdir -p "$dst"
  for file in "$src"/*.scss; do
    [[ -e "$file" ]] || continue
    base="$(basename "$file" .scss)"
    printf '/* compiled by fake sass */\nbody{color:#fff}\n' > "$dst/$base.css"
  done
else
  mkdir -p "$(dirname "$dst")"
  printf '/* compiled by fake sass */\nbody{color:#fff}\n' > "$dst"
fi
"#;
    fs::write(&sass_path, sass_script).expect("write fake sass");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&sass_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&sass_path, perms).expect("chmod fake sass");
    }

    let exe = env!("CARGO_BIN_EXE_fuse");
    let mut cmd = Command::new(exe);
    cmd.arg("build").arg("--manifest-path").arg(&dir);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{}", bin_dir.display(), path));
    let output = cmd.output().expect("run fuse build");
    if !output.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    let built_css = dir.join("public").join("css").join("app.css");
    assert!(built_css.exists(), "expected {}", built_css.display());
    let css = fs::read_to_string(&built_css).expect("read built css");
    assert!(css.contains("compiled by fake sass"), "css: {css}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_hashes_css_outputs_and_writes_asset_manifest() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"

[assets]
scss = "assets/scss"
css = "public/css"
watch = true
hash = true
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");

    let main_src = r#"
app "Demo":
  print("ok")
"#;
    fs::write(dir.join("main.fuse"), main_src).expect("write main.fuse");

    let scss_dir = dir.join("assets").join("scss");
    fs::create_dir_all(&scss_dir).expect("create scss dir");
    fs::write(
        scss_dir.join("app.scss"),
        "$c: #fff;\nbody { color: $c; }\n",
    )
    .expect("write scss");

    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let sass_path = bin_dir.join("sass");
    let sass_script = r#"#!/usr/bin/env bash
set -euo pipefail
mapping=""
for arg in "$@"; do
  case "$arg" in
    --*) ;;
    *) mapping="$arg" ;;
  esac
done
src="${mapping%%:*}"
dst="${mapping#*:}"
if [[ -d "$src" ]]; then
  mkdir -p "$dst"
  for file in "$src"/*.scss; do
    [[ -e "$file" ]] || continue
    base="$(basename "$file" .scss)"
    printf '/* compiled by fake sass */\nbody{color:#fff}\n' > "$dst/$base.css"
  done
else
  mkdir -p "$(dirname "$dst")"
  printf '/* compiled by fake sass */\nbody{color:#fff}\n' > "$dst"
fi
"#;
    fs::write(&sass_path, sass_script).expect("write fake sass");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&sass_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&sass_path, perms).expect("chmod fake sass");
    }

    let exe = env!("CARGO_BIN_EXE_fuse");
    let mut cmd = Command::new(exe);
    cmd.arg("build").arg("--manifest-path").arg(&dir);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{}", bin_dir.display(), path));
    let output = cmd.output().expect("run fuse build");
    if !output.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    let unhashed = dir.join("public").join("css").join("app.css");
    assert!(!unhashed.exists(), "did not expect {}", unhashed.display());

    let css_dir = dir.join("public").join("css");
    let mut hashed = Vec::new();
    for entry in fs::read_dir(&css_dir).expect("read css dir").flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("app.") && name.ends_with(".css") {
            hashed.push(path);
        }
    }
    assert_eq!(hashed.len(), 1, "hashed css files: {hashed:?}");
    let hashed_name = hashed[0]
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    assert!(
        hashed_name.len() > "app..css".len(),
        "unexpected hashed file: {hashed_name}"
    );

    let manifest_path = dir.join(".fuse").join("assets-manifest.json");
    let asset_manifest = fs::read_to_string(&manifest_path).expect("read asset manifest");
    assert!(
        asset_manifest.contains("\"css/app.css\""),
        "manifest: {asset_manifest}"
    );
    let expected_hashed_href = format!("\"/css/{hashed_name}\"");
    assert!(
        asset_manifest.contains(&expected_hashed_href),
        "manifest: {asset_manifest}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_runs_before_build_hook_when_configured() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"

[assets.hooks]
before_build = "fuse-hook-test"
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    let marker = dir.join("hook.marker");
    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let hook_path = bin_dir.join("fuse-hook-test");
    fs::write(
        &hook_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
printf 'ran\n' > "$HOOK_MARKER"
"#,
    )
    .expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).expect("chmod hook");
    }

    let exe = env!("CARGO_BIN_EXE_fuse");
    let mut cmd = Command::new(exe);
    cmd.arg("build").arg("--manifest-path").arg(&dir);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{}", bin_dir.display(), path));
    cmd.env("HOOK_MARKER", marker.to_string_lossy().to_string());
    let output = cmd.output().expect("run fuse build");
    if !output.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    assert!(marker.exists(), "expected {}", marker.display());
    let content = fs::read_to_string(&marker).expect("read marker");
    assert_eq!(content.trim(), "ran");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_fails_when_before_build_hook_fails() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let manifest = r#"
[package]
entry = "main.fuse"
app = "Demo"

[assets.hooks]
before_build = "fuse-hook-fail"
"#;
    fs::write(dir.join("fuse.toml"), manifest).expect("write fuse.toml");
    fs::write(
        dir.join("main.fuse"),
        r#"
app "Demo":
  print("ok")
"#,
    )
    .expect("write main.fuse");

    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let hook_path = bin_dir.join("fuse-hook-fail");
    fs::write(
        &hook_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "hook exploded" >&2
exit 42
"#,
    )
    .expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).expect("chmod hook");
    }

    let exe = env!("CARGO_BIN_EXE_fuse");
    let mut cmd = Command::new(exe);
    cmd.arg("build").arg("--manifest-path").arg(&dir);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{}", bin_dir.display(), path));
    let output = cmd.output().expect("run fuse build");
    assert!(!output.status.success(), "build unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("asset hook error: before_build failed"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("hook exploded"), "stderr: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}
