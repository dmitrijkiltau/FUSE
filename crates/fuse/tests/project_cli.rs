use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_project_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    dir.push(format!("fuse_project_cli_test_{nanos}_{counter}_{pid}"));
    dir
}

#[derive(Debug, Deserialize, Serialize)]
struct TestIrMeta {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    native_cache_version: u32,
    #[serde(default)]
    files: Vec<TestIrFileMeta>,
    #[serde(default)]
    manifest_hash: Option<String>,
    #[serde(default)]
    lock_hash: Option<String>,
    #[serde(default)]
    build_target: String,
    #[serde(default)]
    rustc_version: String,
    #[serde(default)]
    cli_version: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct TestIrFileMeta {
    path: String,
    #[serde(default)]
    hash: String,
}

fn is_hex_sha1(raw: &str) -> bool {
    raw.len() == 40 && raw.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn write_basic_manifest_project(dir: &Path, main_source: &str) {
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"
"#,
    )
    .expect("write fuse.toml");
    fs::write(dir.join("main.fuse"), main_source).expect("write main.fuse");
}

fn run_build_project(dir: &Path) {
    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(dir)
        .output()
        .expect("run fuse build");
    if !build.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&build.stderr));
    }
}

fn run_with_named_arg(dir: &Path, arg: &str) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fuse");
    Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(dir)
        .arg("--")
        .arg(arg)
        .output()
        .expect("run fuse run with args")
}

fn run_with_stdin(dir: &Path, stdin_text: &str) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fuse");
    let mut child = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run fuse run with stdin");
    {
        let mut stdin = child.stdin.take().expect("missing child stdin");
        stdin
            .write_all(stdin_text.as_bytes())
            .expect("write run stdin");
    }
    child.wait_with_output().expect("wait fuse run with stdin")
}

fn write_broken_project(dir: &Path) {
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
app "Demo":
  let id: Missing = 1
"#,
    )
    .expect("write main.fuse");
}

fn write_logging_project(dir: &Path) {
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
app "Demo":
  log("info", "runtime-log")
"#,
    )
    .expect("write main.fuse");
}

fn contains_ansi(raw: &str) -> bool {
    raw.contains("\u{1b}[")
}

fn overwrite_cached_ir_from_source(dir: &Path, source: &str) {
    let source_path = dir.join("__cache_override__.fuse");
    fs::write(&source_path, source).expect("write cache override source");
    let (registry, diags) = fusec::load_program_with_modules(&source_path, source);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let ir = fusec::ir::lower::lower_registry(&registry).expect("lower cache override source");
    let ir_bytes = bincode::serialize(&ir).expect("encode cache override ir");
    fs::write(dir.join(".fuse").join("build").join("program.ir"), ir_bytes)
        .expect("write cache override ir");
    let _ = fs::remove_file(source_path);
}

fn read_program_meta(dir: &Path) -> TestIrMeta {
    let meta_path = dir.join(".fuse").join("build").join("program.meta");
    let bytes = fs::read(&meta_path).expect("read program.meta");
    bincode::deserialize(&bytes).expect("decode program.meta")
}

fn write_program_meta(dir: &Path, meta: &TestIrMeta) {
    let meta_path = dir.join(".fuse").join("build").join("program.meta");
    let bytes = bincode::serialize(meta).expect("encode program.meta");
    fs::write(meta_path, bytes).expect("write program.meta");
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

#[test]
fn build_writes_hash_based_meta_v3() {
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
        dir.join("fuse.lock"),
        r#"
version = 1
"#,
    )
    .expect("write fuse.lock");
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
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse build");
    if !output.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    let meta = read_program_meta(&dir);

    assert_eq!(meta.version, 3, "meta: {meta:?}");
    assert!(meta.native_cache_version > 0, "meta: {meta:?}");
    assert!(
        meta.manifest_hash.as_deref().is_some_and(is_hex_sha1),
        "meta: {meta:?}"
    );
    assert!(
        meta.lock_hash.as_deref().is_some_and(is_hex_sha1),
        "meta: {meta:?}"
    );
    assert!(
        !meta.build_target.trim().is_empty(),
        "meta target fingerprint missing: {meta:?}"
    );
    assert!(
        !meta.rustc_version.trim().is_empty(),
        "meta rustc fingerprint missing: {meta:?}"
    );
    assert!(
        !meta.cli_version.trim().is_empty(),
        "meta cli fingerprint missing: {meta:?}"
    );
    assert!(!meta.files.is_empty(), "meta: {meta:?}");
    for file in &meta.files {
        assert!(!file.path.is_empty(), "meta file entry: {file:?}");
        assert!(is_hex_sha1(&file.hash), "meta file entry: {file:?}");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_invalidates_cached_ir_when_meta_target_fingerprint_changes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let source_program = r#"
fn main(name: String = "world"):
  print("source-" + name)

app "Demo":
  main()
"#;
    write_basic_manifest_project(&dir, source_program);
    run_build_project(&dir);

    let cached_program = r#"
fn main(name: String = "world"):
  print("cache-" + name)

app "Demo":
  main()
"#;
    overwrite_cached_ir_from_source(&dir, cached_program);

    let mut meta = read_program_meta(&dir);
    meta.build_target.push_str("-tampered");
    write_program_meta(&dir, &meta);

    let run = run_with_named_arg(&dir, "--name=fingerprint");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "source-fingerprint"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn run_rebuilds_when_content_changes_even_if_mtime_is_preserved() {
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

    let before = r#"
fn main():
  print("old")

app "Demo":
  main()
"#;
    let after = r#"
fn main():
  print("new")

app "Demo":
  main()
"#;
    assert_eq!(before.len(), after.len(), "fixture length mismatch");
    fs::write(dir.join("main.fuse"), before).expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse build");
    if !build.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&build.stderr));
    }

    let stamp = dir.join("mtime.stamp");
    fs::write(&stamp, "stamp").expect("write mtime stamp");
    let touch_stamp = Command::new("touch")
        .arg("-r")
        .arg(dir.join("main.fuse"))
        .arg(&stamp)
        .output()
        .expect("touch stamp");
    assert!(
        touch_stamp.status.success(),
        "touch stamp stderr: {}",
        String::from_utf8_lossy(&touch_stamp.stderr)
    );

    fs::write(dir.join("main.fuse"), after).expect("rewrite main.fuse");
    let touch_main = Command::new("touch")
        .arg("-r")
        .arg(&stamp)
        .arg(dir.join("main.fuse"))
        .output()
        .expect("touch main");
    assert!(
        touch_main.status.success(),
        "touch main stderr: {}",
        String::from_utf8_lossy(&touch_main.stderr)
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
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "new");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_accepts_program_args_after_build() {
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
fn main(name: String = "world"):
  print(name)

app "Demo":
  main()
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let build = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse build");
    if !build.status.success() {
        panic!("stderr: {}", String::from_utf8_lossy(&build.stderr));
    }

    let run = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .arg("--")
        .arg("--name=cache")
        .output()
        .expect("run fuse run with args");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "cache");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_supports_input_builtin_with_piped_stdin() {
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
app "Demo":
  let name = input("Name: ")
  print("Hello, " + name)
"#,
    )
    .expect("write main.fuse");

    let run = run_with_stdin(&dir, "Codex\n");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "Name: Hello, Codex\n");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_reports_clear_error_when_input_has_no_stdin_data() {
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
app "Demo":
  let _ = input()
"#,
    )
    .expect("write main.fuse");

    let exe = env!("CARGO_BIN_EXE_fuse");
    let run = Command::new(exe)
        .arg("run")
        .arg("--manifest-path")
        .arg(&dir)
        .output()
        .expect("run fuse run");
    assert!(!run.status.success(), "run unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("input requires stdin data in non-interactive mode"),
        "run stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_with_program_args_uses_cached_ir_when_valid() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let source_program = r#"
fn main(name: String = "world"):
  print("source-" + name)

app "Demo":
  main()
"#;
    write_basic_manifest_project(&dir, source_program);
    run_build_project(&dir);

    let cached_program = r#"
fn main(name: String = "world"):
  print("cache-" + name)

app "Demo":
  main()
"#;
    overwrite_cached_ir_from_source(&dir, cached_program);

    let run = run_with_named_arg(&dir, "--name=hit");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "cache-hit");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_invalidates_cached_ir_when_manifest_changes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let source_program = r#"
fn main(name: String = "world"):
  print("source-" + name)

app "Demo":
  main()
"#;
    write_basic_manifest_project(&dir, source_program);
    run_build_project(&dir);

    let cached_program = r#"
fn main(name: String = "world"):
  print("cache-" + name)

app "Demo":
  main()
"#;
    overwrite_cached_ir_from_source(&dir, cached_program);

    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "Demo"
# manifest hash bump
"#,
    )
    .expect("rewrite fuse.toml");

    let run = run_with_named_arg(&dir, "--name=manifest");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "source-manifest"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_invalidates_cached_ir_when_lockfile_changes() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    let source_program = r#"
fn main(name: String = "world"):
  print("source-" + name)

app "Demo":
  main()
"#;
    write_basic_manifest_project(&dir, source_program);
    fs::write(
        dir.join("fuse.lock"),
        r#"
version = 1
"#,
    )
    .expect("write fuse.lock");
    run_build_project(&dir);

    let cached_program = r#"
fn main(name: String = "world"):
  print("cache-" + name)

app "Demo":
  main()
"#;
    overwrite_cached_ir_from_source(&dir, cached_program);

    fs::write(
        dir.join("fuse.lock"),
        r#"
version = 2
"#,
    )
    .expect("rewrite fuse.lock");

    let run = run_with_named_arg(&dir, "--name=lock");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "source-lock");

    let _ = fs::remove_dir_all(&dir);
}

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

    let _ = fs::remove_dir_all(&dir);
}

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
