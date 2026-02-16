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
    let completion = lsp.request("textDocument/completion", completion_params(&main_uri, line, col));
    let completion_text = json::encode(&completion);

    let local_sort = completion_sort_text(&completion, "current")
        .expect("missing local variable completion for current");
    let imported_sort = completion_sort_text(&completion, "greet")
        .expect("missing imported symbol completion for greet");
    let external_sort = completion_sort_text(&completion, "helper")
        .expect("missing external module completion for helper");
    let builtin_sort = completion_sort_text(&completion, "print")
        .expect("missing builtin completion for print");
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
