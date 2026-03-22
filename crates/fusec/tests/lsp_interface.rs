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

fn symbol_names(result: &JsonValue) -> Vec<String> {
    let JsonValue::Array(items) = result else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let JsonValue::Object(symbol) = item else {
            continue;
        };
        if let Some(JsonValue::String(name)) = symbol.get("name") {
            out.push(name.clone());
        }
    }
    out
}

#[test]
fn lsp_indexes_interfaces_and_defines_impl_headers() {
    let dir = temp_project_dir("fuse_lsp_interface");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    let src = r#"interface Encodable:
  fn encode() -> String

type User:
  name: String

impl Encodable for User:
  fn encode() -> String:
    return self.name
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let mut symbol_params = BTreeMap::new();
    symbol_params.insert("query".to_string(), JsonValue::String("Encodable".to_string()));
    let symbols = lsp.request("workspace/symbol", JsonValue::Object(symbol_params));
    let names = symbol_names(&symbols);
    assert!(
        names.iter().any(|name| name == "Encodable"),
        "workspace symbols missing interface: {symbols:?}"
    );

    let (line, col) = line_col_of(src, "Encodable for User");
    let definition = lsp.request(
        "textDocument/definition",
        position_params(&main_uri, line, col + 1),
    );
    let definition_text = json::encode(&definition);
    assert!(
        definition_text.contains(&main_uri) && definition_text.contains("\"line\":0"),
        "definition should jump to interface declaration: {definition_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_reports_reserved_keyword_diagnostics_for_interface_surface() {
    let dir = temp_project_dir("fuse_lsp_interface_diag");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    let src = r#"fn interface(impl: Int) -> Int:
  return impl
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    let diagnostics = lsp.wait_diagnostics(&main_uri);
    let diagnostics_text = json::encode(&JsonValue::Array(diagnostics));
    assert!(
        diagnostics_text.contains("FUSE_RESERVED_KEYWORD"),
        "expected reserved keyword diagnostic, got {diagnostics_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
