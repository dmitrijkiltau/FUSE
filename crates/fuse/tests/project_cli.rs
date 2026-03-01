use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static PORT_COUNTER: AtomicU64 = AtomicU64::new(0);

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

fn run_check_project(dir: &Path) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fuse");
    Command::new(exe)
        .arg("check")
        .arg("--manifest-path")
        .arg(dir)
        .output()
        .expect("run fuse check")
}

fn write_minimal_check_project(dir: &Path, dependencies_block: &str) {
    fs::write(
        dir.join("fuse.toml"),
        format!(
            r#"
[package]
entry = "main.fuse"
app = "Demo"

{dependencies_block}
"#
        ),
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
}

fn write_single_helper_dep_project(dir: &Path) -> String {
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

    let canonical = fs::canonicalize(&helper_dir).expect("canonicalize helper path");
    format!("path:{}", canonical.display())
}

fn create_local_git_dep_repo(base_dir: &Path) -> (String, String) {
    let repo_dir = base_dir.join("git_dep_repo");
    fs::create_dir_all(&repo_dir).expect("create git dep repo");
    fs::write(
        repo_dir.join("fuse.toml"),
        r#"
[package]
entry = "lib.fuse"
app = "GitHelper"
"#,
    )
    .expect("write git dep manifest");
    fs::write(
        repo_dir.join("lib.fuse"),
        "fn helper() -> Int:\n  return 1\n",
    )
    .expect("write git dep lib");

    let init = Command::new("git")
        .arg("init")
        .arg(&repo_dir)
        .output()
        .expect("git init");
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
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
        .arg(&repo_dir)
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
    let rev = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .expect("git rev-parse");
    assert!(
        rev.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&rev.stderr)
    );
    let rev = String::from_utf8_lossy(&rev.stdout).trim().to_string();

    (format!("file://{}", repo_dir.display()), rev)
}

fn extract_lock_string_field(lock_text: &str, key: &str) -> String {
    let prefix = format!("{key} = \"");
    for line in lock_text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&prefix) {
            if let Some(value) = rest.strip_suffix('"') {
                return value.to_string();
            }
        }
    }
    panic!("missing {key} in lockfile: {lock_text}");
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
    let native = fusec::native::compile_registry(&{
        let (registry, diags) = fusec::load_program_with_modules(&source_path, source);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        registry
    })
    .expect("compile cache override native");
    let native_bytes = bincode::serialize(&native).expect("encode cache override native");
    fs::write(
        dir.join(".fuse").join("build").join("program.native"),
        native_bytes,
    )
    .expect("write cache override native");
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

fn default_aot_binary_path(dir: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "program.aot.exe"
    } else {
        "program.aot"
    };
    dir.join(".fuse").join("build").join(name)
}

fn reserve_local_port() -> u16 {
    const PORT_START: u16 = 20_000;
    const PORT_SPAN: u16 = 30_000;
    let pid_offset = (std::process::id() as u16) % PORT_SPAN;
    for _ in 0..PORT_SPAN {
        let seq = PORT_COUNTER.fetch_add(1, Ordering::Relaxed) as u16;
        let candidate = PORT_START + (pid_offset.wrapping_add(seq) % PORT_SPAN);
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", candidate)) {
            drop(listener);
            return candidate;
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test port");
    listener.local_addr().expect("read local test addr").port()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn resolve_target_dir_for_tests() -> PathBuf {
    let root = workspace_root();
    if let Some(raw) = std::env::var_os("CARGO_TARGET_DIR") {
        let path = PathBuf::from(raw);
        return if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
    }
    let tmp_target = root.join("tmp").join("fuse-target");
    if tmp_target.exists() {
        return tmp_target;
    }
    root.join("target")
}

fn find_latest_rlib_for_tests(dir: &Path, prefix: &str) -> PathBuf {
    let entries = fs::read_dir(dir).expect("read deps dir");
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in entries {
        let entry = entry.expect("read deps entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rlib") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !file_name.starts_with(prefix) {
            continue;
        }
        let modified = entry
            .metadata()
            .expect("deps metadata")
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        match best {
            Some((best_time, _)) if modified <= best_time => {}
            _ => best = Some((modified, path)),
        }
    }
    best.expect("find latest rlib").1
}

fn native_link_search_paths_for_tests(target_dir: &Path, profile: &str) -> Vec<PathBuf> {
    let build_dir = target_dir.join(profile).join("build");
    if !build_dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let entries = fs::read_dir(&build_dir).expect("read build dir");
    for entry in entries {
        let path = entry.expect("read build entry").path().join("output");
        if !path.exists() {
            continue;
        }
        let contents = fs::read_to_string(path).expect("read build output metadata");
        for line in contents.lines() {
            let Some(search) = line.strip_prefix("cargo:rustc-link-search=") else {
                continue;
            };
            let search = search.trim();
            if search.is_empty() {
                continue;
            }
            out.push(PathBuf::from(search));
        }
    }
    out
}

fn relink_aot_runner_for_tests(dir: &Path, release: bool) {
    let profile = if release { "release" } else { "debug" };
    let target_dir = resolve_target_dir_for_tests();
    let deps_dir = target_dir.join(profile).join("deps");
    let fusec_rlib = find_latest_rlib_for_tests(&deps_dir, "libfusec");
    let bincode_rlib = find_latest_rlib_for_tests(&deps_dir, "libbincode");
    let runner = dir.join(".fuse").join("build").join("native_main.rs");
    let object = dir.join(".fuse").join("build").join("program.o");
    let output = default_aot_binary_path(dir);

    let mut rustc = Command::new("rustc");
    rustc
        .arg("--edition=2024")
        .arg(&runner)
        .arg("-o")
        .arg(&output)
        .arg("-L")
        .arg(format!("dependency={}", deps_dir.display()))
        .arg("--extern")
        .arg(format!("fusec={}", fusec_rlib.display()))
        .arg("--extern")
        .arg(format!("bincode={}", bincode_rlib.display()))
        .arg("-C")
        .arg(format!("link-arg={}", object.display()));
    for search in native_link_search_paths_for_tests(&target_dir, profile) {
        rustc.arg("-L").arg(search);
    }
    if release {
        rustc.arg("-C").arg("opt-level=3");
    }
    let output = rustc.output().expect("run rustc for relink");
    assert!(
        output.status.success(),
        "relink stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn http_get_with_retry(port: u16, path: &str, attempts: usize) -> Option<String> {
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    http_request_with_retry(port, &request, attempts)
}

fn http_request_with_retry(port: u16, request: &str, attempts: usize) -> Option<String> {
    for _ in 0..attempts {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            if stream.write_all(request.as_bytes()).is_ok() {
                let mut response = String::new();
                if stream.read_to_string(&mut response).is_ok()
                    && !response.is_empty()
                    && response_is_success(&response)
                {
                    return Some(response);
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    None
}

fn response_is_success(response: &str) -> bool {
    let Some(line) = response.lines().next() else {
        return false;
    };
    let Some(code) = line.split_whitespace().nth(1) else {
        return false;
    };
    let Ok(code) = code.parse::<u16>() else {
        return false;
    };
    (200..300).contains(&code)
}

fn wait_for_child_exit_status(child: &mut Child, timeout: Duration) -> ExitStatus {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    panic!("timed out waiting for child exit");
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => panic!("failed to poll child status: {err}"),
        }
    }
}

fn wait_for_dev_ws_url(
    lines: &mpsc::Receiver<String>,
    timeout: Duration,
    stderr_log: &mut String,
) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        let line = match lines.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        };
        stderr_log.push_str(&line);
        stderr_log.push('\n');
        if let Some((_, ws_url)) = line.split_once("live reload websocket: ") {
            return Some(ws_url.trim().to_string());
        }
    }
    None
}

fn connect_reload_websocket(ws_url: &str) -> Result<TcpStream, String> {
    let raw = ws_url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("invalid ws url: {ws_url}"))?;
    let (addr, path_tail) = raw
        .split_once('/')
        .ok_or_else(|| format!("invalid ws url path: {ws_url}"))?;
    let path = format!("/{}", path_tail);
    let mut stream =
        TcpStream::connect(addr).map_err(|err| format!("connect websocket {addr}: {err}"))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGVzdC1mdXNlLWRldi1rZXk=\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("write websocket handshake: {err}"))?;
    let header = read_http_response_header(&mut stream)
        .map_err(|err| format!("read ws handshake: {err}"))?;
    if !header.starts_with("HTTP/1.1 101") {
        return Err(format!("websocket handshake failed: {header}"));
    }
    Ok(stream)
}

fn read_http_response_header(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let read = stream.read(&mut temp)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() >= 16 * 1024 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buffer).to_string())
}

fn read_websocket_text_frame(stream: &mut TcpStream) -> Result<String, String> {
    let mut header = [0u8; 2];
    stream
        .read_exact(&mut header)
        .map_err(|err| format!("read ws header: {err}"))?;
    let masked = (header[1] & 0x80) != 0;
    let mut len = (header[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream
            .read_exact(&mut ext)
            .map_err(|err| format!("read ws ext16: {err}"))?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream
            .read_exact(&mut ext)
            .map_err(|err| format!("read ws ext64: {err}"))?;
        len = u64::from_be_bytes(ext) as usize;
    }
    let mut mask = [0u8; 4];
    if masked {
        stream
            .read_exact(&mut mask)
            .map_err(|err| format!("read ws mask: {err}"))?;
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .map_err(|err| format!("read ws payload: {err}"))?;
    if masked {
        for (idx, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[idx % 4];
        }
    }
    String::from_utf8(payload).map_err(|err| format!("ws payload utf8: {err}"))
}

#[cfg(unix)]
fn send_unix_signal(pid: u32, signal: &str) {
    let status = Command::new("kill")
        .arg(signal)
        .arg(pid.to_string())
        .status()
        .expect("run kill");
    assert!(
        status.success(),
        "kill {signal} {pid} failed with status {status}"
    );
}

#[path = "project_cli/deps_lock.rs"]
mod deps_lock;
#[path = "project_cli/output_aot.rs"]
mod output_aot;
#[path = "project_cli/run_dev_build.rs"]
mod run_dev_build;
#[path = "project_cli/test_diag.rs"]
mod test_diag;
