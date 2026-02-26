use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn write_temp_program(name: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("{name}_{stamp}.fuse"));
    fs::write(&path, contents).expect("failed to write temp program");
    path
}

fn run_program(backend: &str, path: &PathBuf, slug: &str) -> Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(path)
        .env("APP_SLUG", slug)
        .env(
            "FUSE_CONFIG",
            format!("/tmp/fuse_no_config_{}", std::process::id()),
        );
    cmd.output().expect("failed to run fusec")
}

fn test_program() -> &'static str {
    r#"
fn is_slug(value: String) -> Bool:
  return value != "blocked"

config App:
  slug: String(regex("^[a-z0-9_-]+$"), predicate(is_slug)) = "seed"

fn main():
  print(App.slug)

app "demo":
  main()
"#
}

#[test]
fn refinement_constraints_accept_valid_value_all_backends() {
    let path = write_temp_program("fuse_refine_runtime_ok", test_program());
    for backend in ["ast", "native"] {
        let output = run_program(backend, &path, "good_slug");
        assert!(
            output.status.success(),
            "backend={backend} stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("good_slug"),
            "backend={backend} stdout={stdout}"
        );
    }
}

#[test]
fn refinement_regex_rejects_invalid_value_all_backends() {
    let path = write_temp_program("fuse_refine_runtime_regex_fail", test_program());
    for backend in ["ast", "native"] {
        let output = run_program(backend, &path, "BAD!");
        assert!(
            !output.status.success(),
            "backend={backend} expected failure, stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("value does not match regex"),
            "backend={backend} stderr={stderr}"
        );
        assert!(
            stderr.contains("App.slug"),
            "backend={backend} stderr={stderr}"
        );
    }
}

#[test]
fn refinement_predicate_rejects_invalid_value_all_backends() {
    let path = write_temp_program("fuse_refine_runtime_predicate_fail", test_program());
    for backend in ["ast", "native"] {
        let output = run_program(backend, &path, "blocked");
        assert!(
            !output.status.success(),
            "backend={backend} expected failure, stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("predicate") && stderr.contains("rejected value"),
            "backend={backend} stderr={stderr}"
        );
        assert!(
            stderr.contains("App.slug"),
            "backend={backend} stderr={stderr}"
        );
    }
}
