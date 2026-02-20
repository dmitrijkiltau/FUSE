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

#[test]
fn lsp_member_completion_supports_builtin_chains_and_alias_exports() {
    let dir = temp_project_dir("fuse_lsp_completion_member");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let util_src = r#"type Person:
  name: String

fn greet(user: Person) -> String:
  return user.name
"#;
    let main_src = r#"import util from "./util"

fn main():
  let _query = db.from("notes").se
  let _alias = util.gr
  print("ok")
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
    let _ = lsp.wait_diagnostics(&main_uri);

    let (db_line, db_col) = line_col_of(main_src, "db.from(\"notes\").se");
    let db_completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, db_line, db_col + "db.from(\"notes\").se".len()),
    );
    let db_text = json::encode(&db_completion);
    assert!(
        db_text.contains("\"label\":\"select\"") && db_text.contains("db builtin"),
        "expected db chain completion suggestions, got: {db_text}"
    );

    let (alias_line, alias_col) = line_col_of(main_src, "util.gr");
    let alias_completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, alias_line, alias_col + "util.gr".len()),
    );
    let alias_text = json::encode(&alias_completion);
    assert!(
        alias_text.contains("\"label\":\"greet\""),
        "expected alias export completion for greet, got: {alias_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
