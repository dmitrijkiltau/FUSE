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

fn completion_params(uri: &str, line: usize, character: usize) -> JsonValue {
    position_params(uri, line, character)
}

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

#[test]
fn lsp_impl_completion_offers_missing_interface_member_stubs() {
    let dir = temp_project_dir("fuse_lsp_interface_completion");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    let src = r#"interface Codec:
  fn encode() -> String
  fn decode(text: String) -> Self

type Note:
  text: String

impl Codec for Note:
  fn encode() -> String:
    return self.text
  # stub
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    let _ = lsp.wait_diagnostics(&main_uri);

    let (line, col) = line_col_of(src, "impl Codec for Note:");
    let completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "impl Codec for Note:".len()),
    );
    let completion_text = json::encode(&completion);
    assert!(
        completion_text.contains("\"label\":\"decode\""),
        "missing impl stub completion label: {completion_text}"
    );
    assert!(
        completion_text.contains("fn decode(text: String) -> Self")
            && completion_text.contains("TODO: implement decode"),
        "missing impl stub completion insert text: {completion_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_code_action_generates_impl_skeletons() {
    let dir = temp_project_dir("fuse_lsp_interface_code_action");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    let src = r#"interface Codec:
  fn encode() -> String
  fn decode(text: String) -> Self

type Note:
  text: String
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    let diagnostics = lsp.wait_diagnostics(&main_uri);

    let actions = lsp.request(
        "textDocument/codeAction",
        code_action_params(&main_uri, diagnostics),
    );
    let actions_text = json::encode(&actions);
    assert!(
        actions_text.contains("Generate impl Codec for Note"),
        "missing impl skeleton code action: {actions_text}"
    );
    assert!(
        actions_text.contains("impl Codec for Note:")
            && actions_text.contains("fn decode(text: String) -> Self:")
            && actions_text.contains("TODO: implement encode"),
        "missing impl skeleton edit payload: {actions_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
