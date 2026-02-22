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
