use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
enum RuntimeKind {
    Ast,
    Native,
    Aot,
}

impl RuntimeKind {
    fn label(self) -> &'static str {
        match self {
            RuntimeKind::Ast => "ast",
            RuntimeKind::Native => "native",
            RuntimeKind::Aot => "aot",
        }
    }
}

fn temp_project_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    dir.push(format!("fuse_aot_parity_lock_{nanos}_{counter}_{pid}"));
    dir
}

fn write_manifest_project(dir: &Path, source: &str) {
    fs::write(
        dir.join("fuse.toml"),
        r#"
[package]
entry = "main.fuse"
app = "ParityLock"
"#,
    )
    .expect("write fuse.toml");
    fs::write(dir.join("main.fuse"), source).expect("write main.fuse");
}

fn default_aot_binary_path(dir: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "program.aot.exe"
    } else {
        "program.aot"
    };
    dir.join(".fuse").join("build").join(name)
}

fn build_release_aot(dir: &Path) {
    let exe = env!("CARGO_BIN_EXE_fuse");
    let output = Command::new(exe)
        .arg("build")
        .arg("--manifest-path")
        .arg(dir)
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
    let aot = default_aot_binary_path(dir);
    assert!(aot.exists(), "expected {}", aot.display());
}

fn run_mode(dir: &Path, kind: RuntimeKind, mode: &str, db_url: Option<&str>) -> Output {
    match kind {
        RuntimeKind::Ast | RuntimeKind::Native => {
            let exe = env!("CARGO_BIN_EXE_fuse");
            let backend = kind.label();
            let mut cmd = Command::new(exe);
            cmd.arg("run")
                .arg("--manifest-path")
                .arg(dir)
                .arg("--backend")
                .arg(backend)
                .env("APP_MODE", mode)
                .env("FUSE_COLOR", "never")
                .env("NO_COLOR", "1");
            if let Some(db_url) = db_url {
                cmd.env("FUSE_DB_URL", db_url);
            }
            cmd.output().expect("run fuse backend")
        }
        RuntimeKind::Aot => {
            let aot = default_aot_binary_path(dir);
            let mut cmd = Command::new(&aot);
            cmd.current_dir(dir)
                .env("APP_MODE", mode)
                .env("FUSE_COLOR", "never")
                .env("NO_COLOR", "1");
            if let Some(db_url) = db_url {
                cmd.env("FUSE_DB_URL", db_url);
            }
            cmd.output().expect("run aot binary")
        }
    }
}

fn normalize_text(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).replace("\r\n", "\n")
}

fn assert_success(mode: &str, kind: RuntimeKind, output: &Output) -> (String, String) {
    let stdout = normalize_text(&output.stdout);
    let stderr = normalize_text(&output.stderr);
    assert!(
        output.status.success(),
        "mode={mode} backend={} stdout={stdout} stderr={stderr}",
        kind.label()
    );
    (stdout, stderr)
}

fn failure_class(stderr: &str) -> &'static str {
    if stderr.contains("class=panic") {
        "panic"
    } else if stderr.contains("class=runtime_fatal") || stderr.contains("assert failed") {
        "runtime_fatal"
    } else {
        "unknown"
    }
}

fn assert_runtime_fatal(
    mode: &str,
    kind: RuntimeKind,
    output: &Output,
    expected_message: &str,
) -> String {
    let stdout = normalize_text(&output.stdout);
    let stderr = normalize_text(&output.stderr);
    assert!(
        !output.status.success(),
        "mode={mode} backend={} expected failure; stdout={stdout} stderr={stderr}",
        kind.label()
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "mode={mode} backend={} status={:?} stderr={stderr}",
        kind.label(),
        output.status
    );
    assert!(
        stderr.contains(expected_message),
        "mode={mode} backend={} stderr={stderr}",
        kind.label()
    );
    let class = failure_class(&stderr);
    assert_eq!(
        class,
        "runtime_fatal",
        "mode={mode} backend={} stderr={stderr}",
        kind.label()
    );
    class.to_string()
}

fn extract_log_line(stderr: &str, marker: &str, kind: RuntimeKind) -> String {
    stderr
        .lines()
        .find(|line| line.contains(marker))
        .unwrap_or_else(|| panic!("backend={} missing marker {marker}; stderr={stderr}", kind.label()))
        .to_string()
}

fn assert_all_equal(label: &str, values: &[(RuntimeKind, String)]) {
    let baseline = &values[0].1;
    for (kind, value) in values.iter().skip(1) {
        assert_eq!(
            value, baseline,
            "{label} diverged for backend={} baseline_backend={} baseline={baseline} actual={value}",
            kind.label(),
            values[0].0.label()
        );
    }
}

#[test]
fn aot_parity_lock_matrix_is_observable_equivalent() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");

    write_manifest_project(
        &dir,
        r#"
requires db

config App:
  mode: String = env("APP_MODE") ?? "spawn"

fn run_spawn():
  let left = spawn:
    40 + 2
  let right = spawn:
    8
  let a = await left
  let b = await right
  print(a + b)

fn run_json():
  print(json.encode(json.decode("{\"b\":2,\"a\":1}")))
  print(json.encode({"arr": [1, 2, 3], "ok": true}))

fn run_log():
  log("info", "text parity")
  log("info", "json parity", 7)
  print("logged")

fn run_error():
  assert(false, "parity-boom")

fn tx_setup():
  db.exec("create table if not exists items (id integer)")

fn tx_prime():
  tx_setup()
  db.exec("delete from items")
  transaction:
    db.exec("insert into items (id) values (1)")
  print("primed")

fn tx_rollback():
  tx_setup()
  transaction:
    db.exec("insert into items (id) values (2)")
    assert(false, "rollback-boom")

fn tx_status():
  tx_setup()
  print(json.encode(db.query("select id from items order by id")))

app "ParityLock":
  if App.mode == "spawn":
    run_spawn()
  if App.mode == "json":
    run_json()
  if App.mode == "log":
    run_log()
  if App.mode == "error":
    run_error()
  if App.mode == "tx-prime":
    tx_prime()
  if App.mode == "tx-rollback":
    tx_rollback()
  if App.mode == "tx-status":
    tx_status()
"#,
    );

    build_release_aot(&dir);

    let runtimes = [RuntimeKind::Ast, RuntimeKind::Native, RuntimeKind::Aot];

    let mut spawn_outputs = Vec::new();
    for kind in runtimes {
        let output = run_mode(&dir, kind, "spawn", None);
        let (stdout, _stderr) = assert_success("spawn", kind, &output);
        spawn_outputs.push((kind, stdout));
    }
    assert_all_equal("spawn stdout", &spawn_outputs);

    let mut json_outputs = Vec::new();
    for kind in runtimes {
        let output = run_mode(&dir, kind, "json", None);
        let (stdout, _stderr) = assert_success("json", kind, &output);
        json_outputs.push((kind, stdout));
    }
    assert_all_equal("json stdout", &json_outputs);

    let mut log_text_lines = Vec::new();
    let mut log_json_lines = Vec::new();
    for kind in runtimes {
        let output = run_mode(&dir, kind, "log", None);
        let (stdout, stderr) = assert_success("log", kind, &output);
        assert_eq!(stdout.trim(), "logged", "backend={} stdout={stdout}", kind.label());
        log_text_lines.push((kind, extract_log_line(&stderr, "text parity", kind)));
        log_json_lines.push((kind, extract_log_line(&stderr, "\"message\":\"json parity\"", kind)));
    }
    assert_all_equal("log text line", &log_text_lines);
    assert_all_equal("log json line", &log_json_lines);

    let mut error_classes = Vec::new();
    for kind in runtimes {
        let output = run_mode(&dir, kind, "error", None);
        let class = assert_runtime_fatal("error", kind, &output, "parity-boom");
        error_classes.push((kind, class));
    }
    assert_all_equal("error class", &error_classes);

    let mut tx_status_outputs = Vec::new();
    let mut tx_rollback_classes = Vec::new();
    for kind in runtimes {
        let db_path = dir.join(format!("tx_{}.sqlite", kind.label()));
        let db_url = format!("sqlite://{}", db_path.display());

        let prime = run_mode(&dir, kind, "tx-prime", Some(&db_url));
        let (prime_stdout, _prime_stderr) = assert_success("tx-prime", kind, &prime);
        assert_eq!(
            prime_stdout.trim(),
            "primed",
            "backend={} stdout={prime_stdout}",
            kind.label()
        );

        let rollback = run_mode(&dir, kind, "tx-rollback", Some(&db_url));
        let rollback_class = assert_runtime_fatal("tx-rollback", kind, &rollback, "rollback-boom");
        tx_rollback_classes.push((kind, rollback_class));

        let status = run_mode(&dir, kind, "tx-status", Some(&db_url));
        let (status_stdout, _status_stderr) = assert_success("tx-status", kind, &status);
        tx_status_outputs.push((kind, status_stdout));
    }
    assert_all_equal("transaction rollback class", &tx_rollback_classes);
    assert_all_equal("transaction status output", &tx_status_outputs);

    let _ = fs::remove_dir_all(&dir);
}
