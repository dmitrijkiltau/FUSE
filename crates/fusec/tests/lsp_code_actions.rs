use std::collections::BTreeMap;
use std::fs;

use fuse_rt::json::{self, JsonValue};

#[path = "support/lsp.rs"]
mod lsp;
use lsp::{LspClient, path_to_uri, temp_project_dir, write_project_file};

fn code_action_params(uri: &str, diagnostics: Vec<JsonValue>) -> JsonValue {
    let mut text_doc = BTreeMap::new();
    text_doc.insert("uri".to_string(), JsonValue::String(uri.to_string()));

    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(0.0));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert("line".to_string(), JsonValue::Number(120.0));
    range_end.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(range_start));
    range.insert("end".to_string(), JsonValue::Object(range_end));

    let mut context = BTreeMap::new();
    context.insert("diagnostics".to_string(), JsonValue::Array(diagnostics));

    let mut params = BTreeMap::new();
    params.insert("textDocument".to_string(), JsonValue::Object(text_doc));
    params.insert("range".to_string(), JsonValue::Object(range));
    params.insert("context".to_string(), JsonValue::Object(context));
    JsonValue::Object(params)
}

#[test]
fn lsp_code_actions_import_config_scaffold_and_organize_idempotence() {
    let dir = temp_project_dir("fuse_lsp_code_actions");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let util_src = r#"fn greet(name: String, times: Int) -> String:
  return "${name} x ${times}"

fn helper(input: String) -> String:
  return input
"#;
    let main_missing = r#"import { helper } from "./util"

config App:
  dbUrl: String = "sqlite:///tmp/dev.db"

fn main():
  let err = NotFound()
  let size = App.dbPoolSize
  print(helper("Ada"))
  print(err)
  print(size)
"#;
    let main_unsorted = r#"import { helper, greet } from "./util"

fn main():
  print(greet(helper("Ada"), 1))
"#;
    let main_sorted = r#"import { greet, helper } from "./util"

fn main():
  print(greet(helper("Ada"), 1))
"#;

    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, util_src);
    write_project_file(&main_path, main_missing);

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&util_uri, util_src, 1);
    lsp.open_document(&main_uri, main_missing, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    let missing_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        !missing_diags.is_empty(),
        "expected missing import/config diagnostics"
    );

    let actions = lsp.request(
        "textDocument/codeAction",
        code_action_params(&main_uri, missing_diags),
    );
    let actions_text = json::encode(&actions);
    assert!(
        actions_text.contains("Import NotFound from std.Error"),
        "missing import quickfix action: {actions_text}"
    );
    assert!(
        actions_text.contains("Add App.dbPoolSize to config"),
        "missing config scaffold quickfix action: {actions_text}"
    );
    assert!(
        actions_text.contains("dbPoolSize: String = \\\"\\\""),
        "config scaffold edit did not include default placeholder: {actions_text}"
    );

    lsp.change_document(&main_uri, main_unsorted, 2);
    let unsorted_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        unsorted_diags.is_empty(),
        "unexpected diagnostics in organize-imports scenario"
    );
    let organize_actions = lsp.request(
        "textDocument/codeAction",
        code_action_params(&main_uri, Vec::new()),
    );
    let organize_text = json::encode(&organize_actions);
    assert!(
        organize_text.contains("Organize imports") && organize_text.contains("greet, helper"),
        "missing organize imports action/edit: {organize_text}"
    );

    lsp.change_document(&main_uri, main_sorted, 3);
    let sorted_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        sorted_diags.is_empty(),
        "unexpected diagnostics in sorted-imports scenario"
    );
    let organize_after_sorted = lsp.request(
        "textDocument/codeAction",
        code_action_params(&main_uri, Vec::new()),
    );
    let organize_after_sorted_text = json::encode(&organize_after_sorted);
    assert!(
        !organize_after_sorted_text.contains("source.organizeImports"),
        "organize imports should be idempotent after sorting: {organize_after_sorted_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
