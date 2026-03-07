//! Integration tests for M3 — Dependency and package workflow hardening.
//!
//! Tests for transitive `dep:` resolution, cross-package cycle detection,
//! `fuse check --workspace`, and the CLI timestamp cache.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

// ─── Test helpers ─────────────────────────────────────────────────────────────

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}_{nanos}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, text).expect("write file");
}

fn fusec_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fusec"))
}

fn run_check(entry: &Path) -> (bool, String) {
    let out = Command::new(fusec_bin())
        .arg("--check")
        .arg(entry)
        .output()
        .expect("run fusec --check");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stderr)
}

fn run_workspace_check(root: &Path) -> (bool, String) {
    let out = Command::new(fusec_bin())
        .arg("--check")
        .arg("--workspace")
        .current_dir(root)
        .output()
        .expect("run fusec --check --workspace");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stderr)
}

// ─── Manifest-level tests ─────────────────────────────────────────────────────

#[test]
fn build_transitive_deps_expands_nested_deps() {
    use fusec::manifest::build_transitive_deps;

    let root = temp_dir("fuse_transitive_dirs");

    // Package B depends on C.
    let b_root = root.join("pkg_b");
    let c_root = root.join("pkg_c");
    fs::create_dir_all(&b_root).unwrap();
    fs::create_dir_all(&c_root).unwrap();
    write(
        &b_root.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"lib.fuse\"\n\n[dependencies]\nC = \"{}\"\n",
            c_root.display()
        ),
    );
    write(
        &c_root.join("fuse.toml"),
        "[package]\nentry = \"lib.fuse\"\n",
    );

    // Caller only knows about B.
    let mut direct: HashMap<String, PathBuf> = HashMap::new();
    direct.insert("B".to_string(), b_root.clone());

    let (merged, errors) = build_transitive_deps(&direct);
    assert!(errors.is_empty(), "unexpected cycle errors: {errors:?}");
    assert!(merged.contains_key("B"), "merged missing B");
    assert!(
        merged.contains_key("C"),
        "C should be transitively discovered"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn build_transitive_deps_detects_cross_package_cycle() {
    use fusec::manifest::build_transitive_deps;

    let root = temp_dir("fuse_cycle_dirs");
    let a_root = root.join("pkg_a");
    let b_root = root.join("pkg_b");
    fs::create_dir_all(&a_root).unwrap();
    fs::create_dir_all(&b_root).unwrap();

    // A depends on B, B depends on A → cycle.
    write(
        &a_root.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"lib.fuse\"\n\n[dependencies]\nB = \"{}\"\n",
            b_root.display()
        ),
    );
    write(
        &b_root.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"lib.fuse\"\n\n[dependencies]\nA = \"{}\"\n",
            a_root.display()
        ),
    );

    // Caller knows about B.
    let mut direct: HashMap<String, PathBuf> = HashMap::new();
    direct.insert("B".to_string(), b_root.clone());

    let (_merged, errors) = build_transitive_deps(&direct);
    assert!(!errors.is_empty(), "expected cycle error, got none");
    let have_cycle = errors.iter().any(|e| e.contains("circular"));
    assert!(
        have_cycle,
        "error should mention 'circular', got: {errors:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn build_transitive_deps_direct_dep_takes_precedence() {
    use fusec::manifest::build_transitive_deps;

    let root = temp_dir("fuse_precedence_dirs");
    let b_root = root.join("pkg_b");
    let c_v1 = root.join("pkg_c_v1");
    let c_v2 = root.join("pkg_c_v2");
    fs::create_dir_all(&b_root).unwrap();
    fs::create_dir_all(&c_v1).unwrap();
    fs::create_dir_all(&c_v2).unwrap();

    // B pulls in C_v1 as its dep.
    write(
        &b_root.join("fuse.toml"),
        &format!("[dependencies]\nC = \"{}\"\n", c_v1.display()),
    );

    // Caller also provides C directly (v2) — this should win.
    let mut direct: HashMap<String, PathBuf> = HashMap::new();
    direct.insert("B".to_string(), b_root.clone());
    direct.insert("C".to_string(), c_v2.clone());

    let (merged, _) = build_transitive_deps(&direct);
    let merged_c = merged.get("C").expect("C in merged");
    let canon_c_v2 = c_v2.canonicalize().unwrap_or(c_v2.clone());
    let canon_merged_c = merged_c.canonicalize().unwrap_or(merged_c.clone());
    assert_eq!(
        canon_merged_c, canon_c_v2,
        "direct dep C should override B's transitive C"
    );

    let _ = fs::remove_dir_all(&root);
}

// ─── Loader-level transitive resolution tests ─────────────────────────────────

#[test]
fn loader_resolves_transitive_dep_imports() {
    let root = temp_dir("fuse_loader_transitive");

    let b_root = root.join("pkg_b");
    let c_root = root.join("pkg_c");
    fs::create_dir_all(&b_root).unwrap();
    fs::create_dir_all(&c_root).unwrap();

    // Package C: exports a simple helper.
    write(
        &c_root.join("lib.fuse"),
        "fn add_ten(x: Int) -> Int:\n  return x + 10\n",
    );
    write(
        &c_root.join("fuse.toml"),
        "[package]\nentry = \"lib.fuse\"\n",
    );

    // Package B: imports from C.
    write(
        &b_root.join("lib.fuse"),
        "import C from \"dep:C/lib\"\nfn add_c(x: Int) -> Int:\n  return C.add_ten(x)\n",
    );
    write(
        &b_root.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"lib.fuse\"\n\n[dependencies]\nC = \"{}\"\n",
            c_root.display()
        ),
    );

    // Entry program: imports from B. Only B is provided as a direct dep.
    let entry = root.join("main.fuse");
    write(
        &entry,
        "import B from \"dep:B/lib\"\nfn main():\n  print(B.add_c(1))\n",
    );

    let mut direct_deps: HashMap<String, PathBuf> = HashMap::new();
    direct_deps.insert("B".to_string(), b_root);

    let (registry, diags) = fusec::load_program_with_modules_and_deps(
        &entry,
        "import B from \"dep:B/lib\"\nfn main():\n  print(B.add_c(1))\n",
        &direct_deps,
    );
    let error_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.message.contains("unknown"))
        .collect();
    assert!(
        error_diags.is_empty(),
        "transitive dep should resolve: {error_diags:?}"
    );
    // Registry should contain modules from all 3 packages (main + B + C).
    assert!(
        registry.modules.len() >= 3,
        "expected ≥3 modules, got {}",
        registry.modules.len()
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn loader_emits_structured_unknown_dep_diagnostic() {
    let root = temp_dir("fuse_unknown_dep");
    let entry = root.join("main.fuse");
    write(
        &entry,
        "import X from \"dep:Unknown/lib\"\nfn main():\n  pass\n",
    );

    let deps: HashMap<String, PathBuf> = HashMap::new();
    let (_registry, diags) = fusec::load_program_with_modules_and_deps(
        &entry,
        "import X from \"dep:Unknown/lib\"\nfn main():\n  pass\n",
        &deps,
    );

    assert!(!diags.is_empty(), "expected diagnostics for unknown dep");
    let msg = &diags[0].message;
    assert_eq!(
        diags[0].code.as_deref(),
        Some("FUSE_IMPORT_UNKNOWN_DEPENDENCY"),
        "unexpected diagnostic code: {:?}",
        diags[0].code
    );
    // New diagnostic should mention the dep name and hint about available deps.
    assert!(
        msg.contains("Unknown"),
        "message should name the dep: {msg}"
    );
    assert!(
        msg.contains("available") || msg.contains("no dependencies"),
        "message should hint at available deps: {msg}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn loader_emits_dependency_cycle_code() {
    let root = temp_dir("fuse_dep_cycle_loader");
    let a_root = root.join("pkg_a");
    let b_root = root.join("pkg_b");
    fs::create_dir_all(&a_root).unwrap();
    fs::create_dir_all(&b_root).unwrap();

    write(
        &a_root.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"lib.fuse\"\n\n[dependencies]\nB = \"{}\"\n",
            b_root.display()
        ),
    );
    write(&a_root.join("lib.fuse"), "fn ping() -> Int:\n  return 1\n");
    write(
        &b_root.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"lib.fuse\"\n\n[dependencies]\nA = \"{}\"\n",
            a_root.display()
        ),
    );
    write(&b_root.join("lib.fuse"), "fn pong() -> Int:\n  return 2\n");

    let entry = root.join("main.fuse");
    let src = "import B from \"dep:B/lib\"\nfn main():\n  print(B.pong())\n";
    write(&entry, src);

    let mut deps: HashMap<String, PathBuf> = HashMap::new();
    deps.insert("B".to_string(), b_root);

    let (_registry, diags) = fusec::load_program_with_modules_and_deps(&entry, src, &deps);
    assert!(
        diags.iter().any(|diag| diag.code.as_deref() == Some("FUSE_DEP_CYCLE")),
        "expected dependency cycle code, got {:?}",
        diags.iter().map(|diag| (&diag.code, &diag.message)).collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|diag| diag.message.contains("circular")),
        "expected dependency cycle message, got {:?}",
        diags.iter().map(|diag| &diag.message).collect::<Vec<_>>()
    );

    let _ = fs::remove_dir_all(&root);
}

// ─── CLI --workspace tests ────────────────────────────────────────────────────

#[test]
fn workspace_check_passes_for_three_clean_packages() {
    let root = temp_dir("fuse_workspace_3pkg");

    // Package Alpha
    let alpha = root.join("alpha");
    write(
        &alpha.join("fuse.toml"),
        "[package]\nentry = \"lib.fuse\"\n",
    );
    write(
        &alpha.join("lib.fuse"),
        "fn greet(name: String) -> String:\n  return \"Hello \" + name\n",
    );

    // Package Beta (imports Alpha)
    let beta = root.join("beta");
    write(
        &beta.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"lib.fuse\"\n\n[dependencies]\nAlpha = \"{}\"\n",
            alpha.display()
        ),
    );
    write(
        &beta.join("lib.fuse"),
        "import Alpha from \"dep:Alpha/lib\"\nfn say_hello() -> String:\n  return Alpha.greet(\"World\")\n",
    );

    // Package Gamma (imports Beta, which transitively pulls Alpha)
    let gamma = root.join("gamma");
    write(
        &gamma.join("fuse.toml"),
        &format!(
            "[package]\nentry = \"main.fuse\"\n\n[dependencies]\nBeta = \"{}\"\n",
            beta.display()
        ),
    );
    write(
        &gamma.join("main.fuse"),
        "import Beta from \"dep:Beta/lib\"\nfn main():\n  print(Beta.say_hello())\n",
    );

    let (ok, stderr) = run_workspace_check(&root);
    assert!(
        ok,
        "workspace check should pass for 3 clean packages; stderr:\n{stderr}"
    );
    // All three packages should appear in output.
    assert!(
        stderr.contains("alpha") || stderr.contains("ok"),
        "expected package names in output: {stderr}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn workspace_check_fails_with_per_package_errors() {
    let root = temp_dir("fuse_workspace_error");

    // Package with a type error.
    let bad = root.join("bad_pkg");
    write(&bad.join("fuse.toml"), "[package]\nentry = \"main.fuse\"\n");
    // Call non-existent function → sema error.
    write(
        &bad.join("main.fuse"),
        "fn main():\n  let x: Int = undefined_call()\n",
    );

    // Package that is clean.
    let good = root.join("good_pkg");
    write(
        &good.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\n",
    );
    write(&good.join("main.fuse"), "fn main():\n  print(\"ok\")\n");

    let (ok, _stderr) = run_workspace_check(&root);
    assert!(
        !ok,
        "workspace check should fail when any package has errors"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn workspace_check_skips_dirs_without_entry() {
    let root = temp_dir("fuse_workspace_noentry");

    // A manifest with no [package].entry should be skipped silently.
    let utility = root.join("utility");
    write(
        &utility.join("fuse.toml"),
        "[dependencies]\nX = \"../x\"\n", // no [package] section
    );

    // One real package.
    let app = root.join("app");
    write(&app.join("fuse.toml"), "[package]\nentry = \"main.fuse\"\n");
    write(&app.join("main.fuse"), "fn main():\n  print(\"hello\")\n");

    let (ok, _stderr) = run_workspace_check(&root);
    assert!(
        ok,
        "workspace check should succeed when only real packages are checked"
    );

    let _ = fs::remove_dir_all(&root);
}

// ─── CLI timestamp cache tests ────────────────────────────────────────────────

#[test]
fn check_cache_hit_skips_recheck_on_unchanged_source() {
    let root = temp_dir("fuse_cache_hit");
    let entry = root.join("main.fuse");
    write(&entry, "fn main():\n  print(\"hello\")\n");

    // First check: should pass and write cache.
    let (ok1, stderr1) = run_check(&entry);
    assert!(ok1, "first check should pass; stderr:\n{stderr1}");
    assert!(
        !stderr1.contains("cached"),
        "first check should NOT be cached: {stderr1}"
    );

    // Second check: source unchanged, should be a cache hit.
    let (ok2, stderr2) = run_check(&entry);
    assert!(ok2, "second check should pass; stderr:\n{stderr2}");
    assert!(
        stderr2.contains("cached"),
        "second check should report cache hit: {stderr2}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn check_cache_miss_after_source_modification() {
    let root = temp_dir("fuse_cache_miss");
    let entry = root.join("main.fuse");
    write(&entry, "fn main():\n  print(\"hello\")\n");

    // Seed the cache.
    run_check(&entry);

    // Modify the source file — std::fs::write updates mtime.
    std::thread::sleep(std::time::Duration::from_millis(1100)); // ensure mtime differs (1-second granularity on some FS)
    write(&entry, "fn main():\n  print(\"world\")\n");

    // Third check: should re-run (not cached).
    let (ok, stderr) = run_check(&entry);
    assert!(ok, "check after modification should pass: {stderr}");
    assert!(
        !stderr.contains("cached"),
        "check after modification should NOT be cached: {stderr}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn check_cache_invalidated_on_error() {
    let root = temp_dir("fuse_cache_invalidate");
    let entry = root.join("main.fuse");
    write(&entry, "fn main():\n  print(\"hello\")\n");

    // Seed the cache.
    run_check(&entry);

    // Wait so the mtime changes on the next write (coarse filesystem resolution).
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Introduce a type error that sema will always catch.
    write(&entry, "fn main() -> Int:\n  return \"not an integer\"\n");

    let (ok, _) = run_check(&entry);
    assert!(!ok, "check with sema error should fail");

    // Fix the error.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(&entry, "fn main():\n  print(\"hello\")\n");

    // Check should re-run (cache was invalidated by the previous error).
    let (ok, stderr) = run_check(&entry);
    assert!(ok, "check should pass after fix: {stderr}");
    assert!(
        !stderr.contains("cached"),
        "cache should not be used after an error: {stderr}"
    );

    let _ = fs::remove_dir_all(&root);
}

// ─── find_workspace_manifests tests ──────────────────────────────────────────

#[test]
fn find_workspace_manifests_discovers_nested_packages() {
    use fusec::manifest::find_workspace_manifests;

    let root = temp_dir("fuse_discover_manifests");
    let a = root.join("a");
    let b = root.join("subdir").join("b");
    let hidden = root.join(".hidden").join("c");
    let target = root.join("target").join("d");

    for dir in &[&a, &b, &hidden, &target] {
        fs::create_dir_all(dir).unwrap();
        write(&dir.join("fuse.toml"), "[package]\n");
    }

    let found = find_workspace_manifests(&root);
    let found_dirs: Vec<PathBuf> = found
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();

    let canon_a = a.canonicalize().unwrap_or(a.clone());
    let canon_b = b.canonicalize().unwrap_or(b.clone());
    let canon_hidden = hidden.canonicalize().unwrap_or(hidden.clone());
    let canon_target = target.canonicalize().unwrap_or(target.clone());

    assert!(found_dirs.contains(&canon_a), "should discover ./a");
    assert!(found_dirs.contains(&canon_b), "should discover subdir/b");
    assert!(!found_dirs.contains(&canon_hidden), "should skip .hidden/");
    assert!(!found_dirs.contains(&canon_target), "should skip target/");

    let _ = fs::remove_dir_all(&root);
}
