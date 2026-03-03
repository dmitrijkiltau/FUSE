use std::collections::BTreeMap;
use std::fs;
use std::time::Instant;

use fuse_rt::json::{self, JsonValue};

#[path = "support/lsp.rs"]
mod lsp;
use lsp::{LspClient, path_to_uri, temp_project_dir, write_project_file};

const COLD_COMPLETION_BUDGET_MS: u128 = 5_000;
const WARM_COMPLETION_BUDGET_MS: u128 = 1_500;
const COLD_NAV_BUDGET_MS: u128 = 6_000;
const WARM_NAV_BUDGET_MS: u128 = 2_500;
const COLD_DIAGNOSTICS_BUDGET_MS: u128 = 6_000;
const WARM_DIAGNOSTICS_BUDGET_MS: u128 = 3_000;

// Latency budgets for the 50-file workspace fixture
const DIAG_INCREMENTAL_BUDGET_MS: u128 = 500;
const COMPLETION_WARM_BUDGET_MS: u128 = 200;
const SYMBOL_SEARCH_BUDGET_MS: u128 = 300;
const PROGRESSIVE_FIRST_DIAG_BUDGET_MS: u128 = 500;
const EDIT_BURST_BUDGET_MS: u128 = 5_000;

fn line_col_of(text: &str, needle: &str) -> (usize, usize) {
    let idx = text.find(needle).expect("needle");
    let line = text[..idx].bytes().filter(|b| *b == b'\n').count();
    let line_start = text[..idx].rfind('\n').map_or(0, |pos| pos + 1);
    let col = text[line_start..idx].chars().count();
    (line, col)
}

fn completion_params(uri: &str, line: usize, character: usize) -> JsonValue {
    let mut text_doc = BTreeMap::new();
    text_doc.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    let mut pos = BTreeMap::new();
    pos.insert("line".to_string(), JsonValue::Number(line as f64));
    pos.insert("character".to_string(), JsonValue::Number(character as f64));
    let mut params = BTreeMap::new();
    params.insert("textDocument".to_string(), JsonValue::Object(text_doc));
    params.insert("position".to_string(), JsonValue::Object(pos));
    JsonValue::Object(params)
}

fn position_params(uri: &str, line: usize, character: usize) -> JsonValue {
    let mut text_doc = BTreeMap::new();
    text_doc.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    let mut pos = BTreeMap::new();
    pos.insert("line".to_string(), JsonValue::Number(line as f64));
    pos.insert("character".to_string(), JsonValue::Number(character as f64));
    let mut params = BTreeMap::new();
    params.insert("textDocument".to_string(), JsonValue::Object(text_doc));
    params.insert("position".to_string(), JsonValue::Object(pos));
    JsonValue::Object(params)
}

fn references_params(
    uri: &str,
    line: usize,
    character: usize,
    include_declaration: bool,
) -> JsonValue {
    let mut params = match position_params(uri, line, character) {
        JsonValue::Object(map) => map,
        _ => unreachable!(),
    };
    let mut context = BTreeMap::new();
    context.insert(
        "includeDeclaration".to_string(),
        JsonValue::Bool(include_declaration),
    );
    params.insert("context".to_string(), JsonValue::Object(context));
    JsonValue::Object(params)
}

fn cancel_params(id: u64) -> JsonValue {
    let mut params = BTreeMap::new();
    params.insert("id".to_string(), JsonValue::Number(id as f64));
    JsonValue::Object(params)
}

fn assert_cancelled_response(raw: JsonValue) {
    let JsonValue::Object(root) = raw else {
        panic!("expected response object");
    };
    let JsonValue::Object(error) = root.get("error").expect("missing error payload") else {
        panic!("expected error object");
    };
    let JsonValue::Number(code) = error.get("code").expect("missing error code") else {
        panic!("expected numeric error code");
    };
    assert_eq!(*code as i64, -32800, "unexpected cancellation error code");
    let JsonValue::String(message) = error.get("message").expect("missing error message") else {
        panic!("expected error message");
    };
    assert!(
        message.contains("request cancelled"),
        "unexpected cancellation message: {message}"
    );
}

#[test]
fn lsp_cancellation_burst_is_handled_without_hanging() {
    let dir = temp_project_dir("fuse_lsp_cancel_burst");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    let main_src = r#"fn main():
  print(1)
"#;
    let main_path = dir.join("main.fuse");
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (line, col) = line_col_of(main_src, "print");
    let params = completion_params(&main_uri, line, col + 2);
    let mut ids = Vec::new();
    for _ in 0..12 {
        let id = lsp.next_request_id();
        lsp.notify("$/cancelRequest", cancel_params(id));
        lsp.send_request_with_id(id, "textDocument/completion", params.clone());
        ids.push(id);
    }
    for id in ids {
        let raw = lsp.wait_raw_response(id);
        assert_cancelled_response(raw);
    }

    let ok = lsp.request("textDocument/completion", params);
    let ok_text = json::encode(&ok);
    assert!(
        ok_text.contains("\"items\""),
        "completion should still respond after cancellation burst: {ok_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_large_workspace_completion_stays_within_budget() {
    let dir = temp_project_dir("fuse_lsp_perf_budget");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let mut util_src = String::new();
    for i in 0..700 {
        util_src.push_str(&format!(
            "fn f{i:04}(value: Int) -> Int:\n  return value + {i}\n\n"
        ));
    }
    let main_src = r#"import util from "./util"

fn main():
  let value = util.f0001(1)
  let second = util.f0002(2)
  let total = value + second
  print(total)
  print(value)
"#;

    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, &util_src);
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let main_uri = path_to_uri(&main_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&util_uri, &util_src, 1);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (line, col) = line_col_of(main_src, "util.f0002");
    let params = completion_params(&main_uri, line, col + "util.".len());

    let cold_start = Instant::now();
    let first = lsp.request("textDocument/completion", params.clone());
    let cold_ms = cold_start.elapsed().as_millis();
    let first_text = json::encode(&first);
    assert!(
        first_text.contains("\"label\":\"f0001\""),
        "expected util member completions: {first_text}"
    );

    let mut warm_max_ms = 0u128;
    for _ in 0..6 {
        let start = Instant::now();
        let _ = lsp.request("textDocument/completion", params.clone());
        let elapsed = start.elapsed().as_millis();
        warm_max_ms = warm_max_ms.max(elapsed);
    }

    assert!(
        cold_ms <= COLD_COMPLETION_BUDGET_MS,
        "cold completion latency {cold_ms}ms exceeded budget {COLD_COMPLETION_BUDGET_MS}ms"
    );
    assert!(
        warm_max_ms <= WARM_COMPLETION_BUDGET_MS,
        "warm completion max latency {warm_max_ms}ms exceeded budget {WARM_COMPLETION_BUDGET_MS}ms"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_large_workspace_navigation_and_references_stay_within_budget() {
    let dir = temp_project_dir("fuse_lsp_perf_nav_budget");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"src/main.fuse\"\napp = \"Demo\"\n\n[dependencies]\nAuth = { path = \"./deps/auth\" }\n",
    );

    let mut util_src = String::new();
    for i in 0..600 {
        util_src.push_str(&format!(
            "fn f{i:04}(value: Int) -> Int:\n  return value + {i}\n\n"
        ));
    }
    util_src.push_str(
        "fn fanout(value: Int) -> Int:\n  let a = f0001(value)\n  let b = f0002(a)\n  return f0003(b)\n",
    );

    let main_src = r#"import util from "./util"
import Core from "root:lib/core"
import Auth from "dep:Auth/lib"

fn main():
  let seed = util.fanout(1)
  let core = Core.plus_one(seed)
  let dep = Auth.plus_one(core)
  print(dep)
"#;
    let core_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;
    let dep_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let main_path = dir.join("src").join("main.fuse");
    let util_path = dir.join("src").join("util.fuse");
    let core_path = dir.join("lib").join("core.fuse");
    let dep_path = dir.join("deps").join("auth").join("lib.fuse");
    write_project_file(&main_path, main_src);
    write_project_file(&util_path, &util_src);
    write_project_file(&core_path, core_src);
    write_project_file(&dep_path, dep_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let util_uri = path_to_uri(&util_path);
    let core_uri = path_to_uri(&core_path);
    let dep_uri = path_to_uri(&dep_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, main_src, 1);
    lsp.open_document(&util_uri, &util_src, 1);
    lsp.open_document(&core_uri, core_src, 1);
    lsp.open_document(&dep_uri, dep_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&core_uri).is_empty());
    assert!(lsp.wait_diagnostics(&dep_uri).is_empty());

    let (core_line, core_col) = line_col_of(main_src, "Core.plus_one(seed)");
    let core_definition_params =
        position_params(&main_uri, core_line, core_col + "Core.".len() + 1);
    let cold_definition_start = Instant::now();
    let core_definition = lsp.request("textDocument/definition", core_definition_params.clone());
    let cold_definition_ms = cold_definition_start.elapsed().as_millis();
    let core_definition_text = json::encode(&core_definition);
    assert!(
        core_definition_text.contains(&core_uri),
        "definition should resolve to root module: {core_definition_text}"
    );

    let mut warm_definition_max_ms = 0u128;
    for _ in 0..6 {
        let start = Instant::now();
        let _ = lsp.request("textDocument/definition", core_definition_params.clone());
        warm_definition_max_ms = warm_definition_max_ms.max(start.elapsed().as_millis());
    }

    let (dep_line, dep_col) = line_col_of(main_src, "Auth.plus_one(core)");
    let dep_references_params =
        references_params(&main_uri, dep_line, dep_col + "Auth.".len() + 1, true);
    let cold_refs_start = Instant::now();
    let dep_refs = lsp.request("textDocument/references", dep_references_params.clone());
    let cold_refs_ms = cold_refs_start.elapsed().as_millis();
    let dep_refs_text = json::encode(&dep_refs);
    assert!(
        dep_refs_text.contains(&main_uri) && dep_refs_text.contains(&dep_uri),
        "references should include callsite and dependency declaration: {dep_refs_text}"
    );

    let mut warm_refs_max_ms = 0u128;
    for _ in 0..6 {
        let start = Instant::now();
        let _ = lsp.request("textDocument/references", dep_references_params.clone());
        warm_refs_max_ms = warm_refs_max_ms.max(start.elapsed().as_millis());
    }

    assert!(
        cold_definition_ms <= COLD_NAV_BUDGET_MS,
        "cold definition latency {cold_definition_ms}ms exceeded budget {COLD_NAV_BUDGET_MS}ms"
    );
    assert!(
        warm_definition_max_ms <= WARM_NAV_BUDGET_MS,
        "warm definition max latency {warm_definition_max_ms}ms exceeded budget {WARM_NAV_BUDGET_MS}ms"
    );
    assert!(
        cold_refs_ms <= COLD_NAV_BUDGET_MS,
        "cold references latency {cold_refs_ms}ms exceeded budget {COLD_NAV_BUDGET_MS}ms"
    );
    assert!(
        warm_refs_max_ms <= WARM_NAV_BUDGET_MS,
        "warm references max latency {warm_refs_max_ms}ms exceeded budget {WARM_NAV_BUDGET_MS}ms"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_large_workspace_diagnostics_stay_within_budget() {
    let dir = temp_project_dir("fuse_lsp_perf_diag_budget");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let mut util_src = String::new();
    for i in 0..700 {
        util_src.push_str(&format!(
            "fn f{i:04}(value: Int) -> Int:\n  return value + {i}\n\n"
        ));
    }
    util_src.push_str(
        "fn keep(value: Int) -> Int:\n  let a = f0001(value)\n  let b = f0002(a)\n  return b\n",
    );
    let main_src = r#"import util from "./util"

fn main():
  print(util.keep(1))
"#;

    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, &util_src);
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let main_uri = path_to_uri(&main_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    let cold_start = Instant::now();
    lsp.open_document(&util_uri, &util_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    let cold_ms = cold_start.elapsed().as_millis();

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let util_src_v2 = format!(
        "{util_src}\nfn keep2(value: Int) -> Int:\n  let next = keep(value)\n  return next\n"
    );
    let warm_start = Instant::now();
    lsp.change_document(&util_uri, &util_src_v2, 2);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    let warm_ms = warm_start.elapsed().as_millis();

    assert!(
        cold_ms <= COLD_DIAGNOSTICS_BUDGET_MS,
        "cold diagnostics latency {cold_ms}ms exceeded budget {COLD_DIAGNOSTICS_BUDGET_MS}ms"
    );
    assert!(
        warm_ms <= WARM_DIAGNOSTICS_BUDGET_MS,
        "incremental diagnostics latency {warm_ms}ms exceeded budget {WARM_DIAGNOSTICS_BUDGET_MS}ms"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

// ─── 50-file workspace latency budget tests ──────────────────────────────────

fn workspace_symbol_params(query: &str) -> JsonValue {
    let mut params = BTreeMap::new();
    params.insert("query".to_string(), JsonValue::String(query.to_string()));
    JsonValue::Object(params)
}

fn workspace_stats_params() -> JsonValue {
    JsonValue::Object(BTreeMap::new())
}

/// Build a 50-file workspace fixture.
/// Returns (dir, main_path, root_uri, main_src).
/// Files file_00.fuse … file_49.fuse each export 3 small functions.
/// main.fuse imports file_00 and file_01.
fn build_50_file_workspace(
    prefix: &str,
) -> (std::path::PathBuf, std::path::PathBuf, String, String) {
    let dir = temp_project_dir(prefix);
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    for i in 0u32..50 {
        let src = format!(
            "fn f{i}_a(x: Int) -> Int:\n  return x + {i}\n\nfn f{i}_b(x: Int) -> Int:\n  return x * {i}\n\nfn f{i}_c(x: Int) -> Int:\n  return x - {i}\n"
        );
        write_project_file(&dir.join(format!("file_{i:02}.fuse")), &src);
    }
    let main_src = r#"import m0 from "./file_00"
import m1 from "./file_01"

fn main():
  let a = m0.f0_a(1)
  let b = m1.f1_b(a)
  print(b)
"#;
    let main_path = dir.join("main.fuse");
    write_project_file(&main_path, main_src);
    let root_uri = path_to_uri(&dir);
    (dir, main_path, root_uri, main_src.to_string())
}

/// Incremental diagnostics for an edit inside a 50-file workspace must arrive
/// within DIAG_INCREMENTAL_BUDGET_MS (500 ms) after the workspace is warm.
#[test]
fn lsp_50_file_workspace_incremental_diagnostics_within_budget() {
    let (dir, main_path, root_uri, main_src) = build_50_file_workspace("fuse_lsp_m4_diag");
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, &main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    // Warm up: ensure full workspace is cached by issuing a workspace/symbol request.
    let _ = lsp.request("workspace/symbol", workspace_symbol_params("f0"));

    // Incremental edit: add a new function to main.fuse.
    let main_src_v2 = format!("{main_src}\nfn extra(x: Int) -> Int:\n  return x + 99\n");
    let edit_start = Instant::now();
    lsp.change_document(&main_uri, &main_src_v2, 2);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    let edit_ms = edit_start.elapsed().as_millis();

    assert!(
        edit_ms <= DIAG_INCREMENTAL_BUDGET_MS,
        "incremental diagnostics latency {edit_ms}ms exceeded budget {DIAG_INCREMENTAL_BUDGET_MS}ms"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// Warm completion inside a 50-file workspace must respond within
/// COMPLETION_WARM_BUDGET_MS (200 ms) once the workspace cache is hot.
#[test]
fn lsp_50_file_workspace_completion_warm_within_budget() {
    let (dir, main_path, root_uri, main_src) = build_50_file_workspace("fuse_lsp_m4_comp");
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, &main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (line, col) = line_col_of(&main_src, "m0.");
    let params = completion_params(&main_uri, line, col + "m0.".len());

    // One cold request to prime the workspace cache.
    let _ = lsp.request("textDocument/completion", params.clone());

    // Warm measurements.
    let mut max_warm_ms = 0u128;
    for _ in 0..6 {
        let start = Instant::now();
        let _ = lsp.request("textDocument/completion", params.clone());
        max_warm_ms = max_warm_ms.max(start.elapsed().as_millis());
    }

    assert!(
        max_warm_ms <= COMPLETION_WARM_BUDGET_MS,
        "warm completion latency {max_warm_ms}ms exceeded budget {COMPLETION_WARM_BUDGET_MS}ms"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// workspace/symbol on a 50-file workspace must respond within
/// SYMBOL_SEARCH_BUDGET_MS (300 ms).
#[test]
fn lsp_50_file_workspace_symbol_search_within_budget() {
    let (dir, main_path, root_uri, main_src) = build_50_file_workspace("fuse_lsp_m4_sym");
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, &main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    // Cold workspace/symbol — builds the full index.
    let cold_start = Instant::now();
    let result = lsp.request("workspace/symbol", workspace_symbol_params("f0"));
    let cold_ms = cold_start.elapsed().as_millis();
    let result_text = fuse_rt::json::encode(&result);
    assert!(
        result_text.contains("f0_a"),
        "workspace/symbol did not return expected results: {result_text}"
    );

    // Warm workspace/symbol — index already cached.
    let warm_start = Instant::now();
    let _ = lsp.request("workspace/symbol", workspace_symbol_params("f0"));
    let warm_ms = warm_start.elapsed().as_millis();

    assert!(
        cold_ms <= SYMBOL_SEARCH_BUDGET_MS,
        "cold workspace/symbol latency {cold_ms}ms exceeded budget {SYMBOL_SEARCH_BUDGET_MS}ms"
    );
    assert!(
        warm_ms <= SYMBOL_SEARCH_BUDGET_MS,
        "warm workspace/symbol latency {warm_ms}ms exceeded budget {SYMBOL_SEARCH_BUDGET_MS}ms"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// Opening a single file in a 50-file workspace must not block on loading all
/// 50 files.  The first diagnostics response must arrive within
/// PROGRESSIVE_FIRST_DIAG_BUDGET_MS and workspace stats must show that
/// only a progressive (focus-file) snapshot was built, not the full workspace.
#[test]
fn lsp_progressive_indexing_does_not_block_on_full_workspace_load() {
    let (dir, _main_path, root_uri, _) = build_50_file_workspace("fuse_lsp_m4_prog");

    // Open a file that is NOT the entry point and has no imports — only that
    // one file needs to be parsed for diagnostics.
    let solo_path = dir.join("file_25.fuse");
    let solo_uri = path_to_uri(&solo_path);
    let solo_src = fs::read_to_string(&solo_path).expect("read file_25.fuse");

    let mut lsp = LspClient::spawn_with_root(&root_uri);

    let first_diag_start = Instant::now();
    lsp.open_document(&solo_uri, &solo_src, 1);
    let diags = lsp.wait_diagnostics(&solo_uri);
    let first_diag_ms = first_diag_start.elapsed().as_millis();

    assert!(
        diags.is_empty(),
        "unexpected diagnostics for file_25.fuse: {diags:?}"
    );
    assert!(
        first_diag_ms <= PROGRESSIVE_FIRST_DIAG_BUDGET_MS,
        "first diagnostics for standalone file took {first_diag_ms}ms, exceeded budget {PROGRESSIVE_FIRST_DIAG_BUDGET_MS}ms"
    );

    // Verify that no full workspace snapshot was built — only a progressive one.
    let stats = lsp.request("fuse/internalWorkspaceStats", workspace_stats_params());
    let stats_text = fuse_rt::json::encode(&stats);
    assert!(
        stats_text.contains("\"workspaceBuilds\":0"),
        "full workspace should not have been built for single-file open: {stats_text}"
    );
    assert!(
        stats_text.contains("\"progressiveBuilds\":1"),
        "progressive snapshot should have been built: {stats_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// Rapid editing bursts in a large workspace must not cause the LSP server to
/// hang.  After the burst, the server must still respond to a normal request.
///
/// Note: cancellation-under-burst behavior is covered separately by
/// `lsp_cancellation_burst_is_handled_without_hanging`.  This test deliberately
/// avoids mixing diagnostics-drain and response-drain loops on the same pipe,
/// which would cause one loop to consume messages intended for the other.
#[test]
fn lsp_large_workspace_edit_burst_does_not_hang() {
    let (dir, main_path, root_uri, main_src) = build_50_file_workspace("fuse_lsp_burst");
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, &main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    // Send 20 rapid edits. Each produces one synchronous publishDiagnostics
    // notification from the server, so draining is straightforward.
    const BURST_SIZE: u32 = 20;
    let burst_start = Instant::now();
    for i in 0..BURST_SIZE {
        let new_src = format!("{main_src}\n// burst edit {i}\n");
        lsp.change_document(&main_uri, &new_src, (i + 2) as u64);
    }
    for _ in 0..BURST_SIZE {
        let _ = lsp.wait_diagnostics(&main_uri);
    }
    let burst_ms = burst_start.elapsed().as_millis();
    assert!(
        burst_ms <= EDIT_BURST_BUDGET_MS,
        "burst drain took {burst_ms}ms, exceeded budget {EDIT_BURST_BUDGET_MS}ms"
    );

    // Server must still respond normally after the burst.
    let (line, col) = line_col_of(&main_src, "m0.");
    let ok = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "m0.".len()),
    );
    let ok_text = fuse_rt::json::encode(&ok);
    assert!(
        ok_text.contains("\"items\""),
        "completion should still respond after edit burst: {ok_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
