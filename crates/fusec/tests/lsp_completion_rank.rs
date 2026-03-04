use std::collections::BTreeMap;
use std::fs;

use fuse_rt::json::{self, JsonValue};

#[path = "support/lsp.rs"]
mod lsp;
use lsp::{LspClient, path_to_uri, temp_project_dir, write_project_file};

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

fn completion_sort_text(result: &JsonValue, label: &str) -> Option<String> {
    let JsonValue::Object(root) = result else {
        return None;
    };
    let JsonValue::Array(items) = root.get("items")? else {
        return None;
    };
    for item in items {
        let JsonValue::Object(item) = item else {
            continue;
        };
        let Some(JsonValue::String(item_label)) = item.get("label") else {
            continue;
        };
        if item_label != label {
            continue;
        }
        if let Some(JsonValue::String(sort_text)) = item.get("sortText") {
            return Some(sort_text.clone());
        }
    }
    None
}

#[test]
fn lsp_completion_ranking_groups_are_stable() {
    let dir = temp_project_dir("fuse_lsp_completion_rank");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let util_src = r#"fn helper(v: String) -> String:
  return v

fn greet(v: String) -> String:
  return v
"#;
    let main_src = r#"import { greet } from "./util"

fn root_fn(input: String) -> String:
  let current = input
  
  return current

fn main():
  print(root_fn("x"))
"#;
    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, util_src);
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&util_uri, util_src, 1);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (line, col) = line_col_of(main_src, "  return current");
    let completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col),
    );
    let completion_text = json::encode(&completion);

    let local_sort = completion_sort_text(&completion, "current")
        .expect("missing local variable completion for current");
    let imported_sort = completion_sort_text(&completion, "greet")
        .expect("missing imported symbol completion for greet");
    let external_sort = completion_sort_text(&completion, "helper")
        .expect("missing external module completion for helper");
    let builtin_sort =
        completion_sort_text(&completion, "print").expect("missing builtin completion for print");
    let keyword_sort =
        completion_sort_text(&completion, "if").expect("missing keyword completion for if");

    assert!(
        local_sort.starts_with("00_"),
        "local should be rank group 00, got {local_sort}; payload: {completion_text}"
    );
    assert!(
        imported_sort.starts_with("01_"),
        "imported should be rank group 01, got {imported_sort}; payload: {completion_text}"
    );
    assert!(
        external_sort.starts_with("02_"),
        "external should be rank group 02, got {external_sort}; payload: {completion_text}"
    );
    assert!(
        builtin_sort.starts_with("03_"),
        "builtin should be rank group 03, got {builtin_sort}; payload: {completion_text}"
    );
    assert!(
        keyword_sort.starts_with("04_"),
        "keyword should be rank group 04, got {keyword_sort}; payload: {completion_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

// Two dependency modules (dep_a, dep_b), each with multiple functions.
// main.fuse imports one function from each.  The remaining functions in those
// modules are in the workspace but not imported.  Assert that:
//   - a local variable in the current function   → group 00
//   - symbols imported into main.fuse            → group 01
//   - workspace symbols not imported into main   → group 02
//   - builtins                                   → group 03
//   - keywords                                   → group 04
#[test]
fn lsp_completion_ranking_stable_with_many_dep_modules() {
    let dir = temp_project_dir("fuse_lsp_rank_many_deps");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let dep_a_src = "fn alpha_fn(v: String) -> String:\n  return v\n\nfn alpha_run(v: String) -> String:\n  return v\n\nfn alpha_process(v: String) -> String:\n  return v\n";
    let dep_b_src = "fn beta_fn(v: String) -> String:\n  return v\n\nfn beta_run(v: String) -> String:\n  return v\n\nfn beta_process(v: String) -> String:\n  return v\n";
    let main_src = "import { alpha_fn } from \"./dep_a\"\nimport { beta_fn } from \"./dep_b\"\n\nfn compute(v: String) -> String:\n  let work_item = v\n\n  return work_item\n\nfn main():\n  print(compute(\"bench\"))\n";

    let dep_a_path = dir.join("dep_a.fuse");
    let dep_b_path = dir.join("dep_b.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&dep_a_path, dep_a_src);
    write_project_file(&dep_b_path, dep_b_src);
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let dep_a_uri = path_to_uri(&dep_a_path);
    let dep_b_uri = path_to_uri(&dep_b_path);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&dep_a_uri, dep_a_src, 1);
    lsp.open_document(&dep_b_uri, dep_b_src, 1);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&dep_a_uri).is_empty());
    assert!(lsp.wait_diagnostics(&dep_b_uri).is_empty());
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    // Trigger completion at the start of "  return work_item" — empty prefix,
    // so all candidates are visible.
    let (line, col) = line_col_of(main_src, "  return work_item");
    let completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col),
    );
    let completion_text = json::encode(&completion);

    let local_sort = completion_sort_text(&completion, "work_item")
        .expect("missing local variable completion for work_item");
    let alpha_fn_sort = completion_sort_text(&completion, "alpha_fn")
        .expect("missing imported completion for alpha_fn");
    let beta_fn_sort = completion_sort_text(&completion, "beta_fn")
        .expect("missing imported completion for beta_fn");
    let alpha_run_sort = completion_sort_text(&completion, "alpha_run")
        .expect("missing workspace completion for alpha_run");
    let beta_run_sort = completion_sort_text(&completion, "beta_run")
        .expect("missing workspace completion for beta_run");
    let builtin_sort =
        completion_sort_text(&completion, "print").expect("missing builtin completion for print");
    let keyword_sort =
        completion_sort_text(&completion, "if").expect("missing keyword completion for if");

    assert!(
        local_sort.starts_with("00_"),
        "local work_item should be rank group 00, got {local_sort}; payload: {completion_text}"
    );
    assert!(
        alpha_fn_sort.starts_with("01_"),
        "imported alpha_fn should be rank group 01, got {alpha_fn_sort}; payload: {completion_text}"
    );
    assert!(
        beta_fn_sort.starts_with("01_"),
        "imported beta_fn should be rank group 01, got {beta_fn_sort}; payload: {completion_text}"
    );
    assert!(
        alpha_run_sort.starts_with("02_"),
        "workspace-only alpha_run should be rank group 02, got {alpha_run_sort}; payload: {completion_text}"
    );
    assert!(
        beta_run_sort.starts_with("02_"),
        "workspace-only beta_run should be rank group 02, got {beta_run_sort}; payload: {completion_text}"
    );
    assert!(
        builtin_sort.starts_with("03_"),
        "builtin print should be rank group 03, got {builtin_sort}; payload: {completion_text}"
    );
    assert!(
        keyword_sort.starts_with("04_"),
        "keyword if should be rank group 04, got {keyword_sort}; payload: {completion_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

// Verify that within rank group 02 (external workspace symbols), locality weighting
// correctly sub-ranks symbols:
//   "02_0_…" — symbol is in a module directly imported by main.fuse
//   "02_1_…" — symbol is only transitively reachable (imported by an import)
//
// Fixture layout:
//   dep_util.fuse  — defines util_fn, util_helper (leaf module)
//   dep_a.fuse     — imports util_fn from dep_util; defines alpha_fn, alpha_extra
//   main.fuse      — imports alpha_fn from dep_a (direct); dep_util is NOT directly imported
//
// Expected sortText:
//   alpha_extra → "02_0_alpha_extra"  (dep_a is directly imported by main)
//   util_fn     → "02_1_util_fn"      (dep_util is only reachable via dep_a)
#[test]
fn lsp_completion_ranking_locality_weights_directly_imported_modules() {
    let dir = temp_project_dir("fuse_lsp_rank_locality");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let dep_util_src = "fn util_fn(v: String) -> String:\n  return v\n\nfn util_helper(v: String) -> String:\n  return v\n";
    let dep_a_src = "import { util_fn } from \"./dep_util\"\n\nfn alpha_fn(v: String) -> String:\n  return util_fn(v)\n\nfn alpha_extra(v: String) -> String:\n  return v\n";
    let main_src = "import { alpha_fn } from \"./dep_a\"\n\nfn run(v: String) -> String:\n  let task = v\n\n  return task\n\nfn main():\n  print(run(\"x\"))\n";

    let dep_util_path = dir.join("dep_util.fuse");
    let dep_a_path = dir.join("dep_a.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&dep_util_path, dep_util_src);
    write_project_file(&dep_a_path, dep_a_src);
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let dep_util_uri = path_to_uri(&dep_util_path);
    let dep_a_uri = path_to_uri(&dep_a_path);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&dep_util_uri, dep_util_src, 1);
    lsp.open_document(&dep_a_uri, dep_a_src, 1);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&dep_util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&dep_a_uri).is_empty());
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (line, col) = line_col_of(main_src, "  return task");
    let completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col),
    );
    let completion_text = json::encode(&completion);

    // alpha_extra is in dep_a.fuse, which is directly imported by main.fuse → locality 0.
    let alpha_extra_sort = completion_sort_text(&completion, "alpha_extra")
        .expect("missing workspace completion for alpha_extra");
    // util_fn is in dep_util.fuse, which is NOT directly imported by main.fuse
    // (only transitively via dep_a) → locality 1.
    let util_fn_sort = completion_sort_text(&completion, "util_fn")
        .expect("missing workspace completion for util_fn");

    assert!(
        alpha_extra_sort.starts_with("02_0_"),
        "alpha_extra (directly-imported module) should be rank 02_0_, got {alpha_extra_sort}; payload: {completion_text}"
    );
    assert!(
        util_fn_sort.starts_with("02_1_"),
        "util_fn (transitively-imported module) should be rank 02_1_, got {util_fn_sort}; payload: {completion_text}"
    );
    // Locality-adjacent must sort before transitively-reachable within the same group.
    assert!(
        alpha_extra_sort < util_fn_sort,
        "locality-adjacent alpha_extra ({alpha_extra_sort}) should sort before transitive util_fn ({util_fn_sort})"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

// Both lib_alpha and lib_beta export a function named shared_proc.
// main.fuse imports shared_proc only from lib_alpha (and beta_exclusive from lib_beta
// to keep lib_beta reachable in the workspace index).
// Assert that shared_proc resolves to rank group 01 (imported wins) rather than 02
// (the non-imported copy from lib_beta), exercising the upsert dedup logic.
#[test]
fn lsp_completion_ranking_imported_wins_name_collision() {
    let dir = temp_project_dir("fuse_lsp_rank_name_collision");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let lib_alpha_src = "fn shared_proc(v: String) -> String:\n  return v\n\nfn alpha_exclusive(v: String) -> String:\n  return v\n";
    let lib_beta_src = "fn shared_proc(v: String) -> String:\n  return v\n\nfn beta_exclusive(v: String) -> String:\n  return v\n";
    let main_src = "import { shared_proc } from \"./lib_alpha\"\nimport { beta_exclusive } from \"./lib_beta\"\n\nfn work(v: String) -> String:\n  let local_result = v\n\n  return local_result\n\nfn main():\n  print(work(\"test\"))\n";

    let lib_alpha_path = dir.join("lib_alpha.fuse");
    let lib_beta_path = dir.join("lib_beta.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&lib_alpha_path, lib_alpha_src);
    write_project_file(&lib_beta_path, lib_beta_src);
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let lib_alpha_uri = path_to_uri(&lib_alpha_path);
    let lib_beta_uri = path_to_uri(&lib_beta_path);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&lib_alpha_uri, lib_alpha_src, 1);
    lsp.open_document(&lib_beta_uri, lib_beta_src, 1);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&lib_alpha_uri).is_empty());
    assert!(lsp.wait_diagnostics(&lib_beta_uri).is_empty());
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (line, col) = line_col_of(main_src, "  return local_result");
    let completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col),
    );
    let completion_text = json::encode(&completion);

    let local_sort = completion_sort_text(&completion, "local_result")
        .expect("missing local variable completion for local_result");
    // shared_proc exists in both lib_alpha (imported → 01) and lib_beta (not imported → 02).
    // The lower rank must win: 01.
    let shared_proc_sort = completion_sort_text(&completion, "shared_proc")
        .expect("missing completion for shared_proc");
    let beta_exclusive_sort = completion_sort_text(&completion, "beta_exclusive")
        .expect("missing imported completion for beta_exclusive");
    // alpha_exclusive is defined in lib_alpha but NOT imported into main.fuse → 02.
    let alpha_exclusive_sort = completion_sort_text(&completion, "alpha_exclusive")
        .expect("missing workspace completion for alpha_exclusive");
    let builtin_sort =
        completion_sort_text(&completion, "print").expect("missing builtin completion for print");
    let keyword_sort =
        completion_sort_text(&completion, "if").expect("missing keyword completion for if");

    assert!(
        local_sort.starts_with("00_"),
        "local_result should be rank group 00, got {local_sort}; payload: {completion_text}"
    );
    assert!(
        shared_proc_sort.starts_with("01_"),
        "shared_proc should be rank group 01 (imported from lib_alpha wins over non-imported from lib_beta), got {shared_proc_sort}; payload: {completion_text}"
    );
    assert!(
        beta_exclusive_sort.starts_with("01_"),
        "beta_exclusive should be rank group 01 (imported), got {beta_exclusive_sort}; payload: {completion_text}"
    );
    assert!(
        alpha_exclusive_sort.starts_with("02_"),
        "alpha_exclusive should be rank group 02 (in lib_alpha but not imported), got {alpha_exclusive_sort}; payload: {completion_text}"
    );
    assert!(
        builtin_sort.starts_with("03_"),
        "print should be rank group 03, got {builtin_sort}; payload: {completion_text}"
    );
    assert!(
        keyword_sort.starts_with("04_"),
        "keyword if should be rank group 04, got {keyword_sort}; payload: {completion_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
