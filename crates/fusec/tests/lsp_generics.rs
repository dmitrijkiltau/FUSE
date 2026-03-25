use std::collections::BTreeMap;
use std::fs;

use fuse_rt::json::{self, JsonValue};

#[path = "support/lsp.rs"]
mod lsp;
use lsp::{LspClient, path_to_uri, temp_project_dir, write_project_file};

fn line_col_of(text: &str, needle: &str) -> (usize, usize) {
    let idx = text.find(needle).expect("needle not found in source");
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

fn signature_help_params(uri: &str, line: usize, character: usize) -> JsonValue {
    position_params(uri, line, character)
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

fn signature_label(result: &JsonValue) -> String {
    let JsonValue::Object(root) = result else {
        panic!("signature help should be object, got {}", json::encode(result));
    };
    let Some(JsonValue::Array(signatures)) = root.get("signatures") else {
        panic!("missing signatures payload: {}", json::encode(result));
    };
    let Some(JsonValue::Object(signature)) = signatures.first() else {
        panic!("missing first signature: {}", json::encode(result));
    };
    match signature.get("label") {
        Some(JsonValue::String(label)) => label.clone(),
        _ => panic!("missing signature label: {}", json::encode(result)),
    }
}

fn completion_labels(result: &JsonValue) -> Vec<String> {
    let items = match result {
        JsonValue::Array(items) => items,
        JsonValue::Object(obj) => {
            if let Some(JsonValue::Array(items)) = obj.get("items") {
                items
            } else {
                return Vec::new();
            }
        }
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in items {
        if let JsonValue::Object(entry) = item {
            if let Some(JsonValue::String(label)) = entry.get("label") {
                out.push(label.clone());
            }
        }
    }
    out
}

/// Signature help on a generic function call shows the type params and `where` clause.
#[test]
fn lsp_signature_help_shows_type_params_and_where_clause() {
    let dir = temp_project_dir("fuse_lsp_generics_sig");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let src = r#"interface Encodable:
  fn encode() -> String
  fn decode(s: String) -> Self

type User:
  name: String

impl Encodable for User:
  fn encode() -> String:
    return self.name
  fn decode(s: String) -> Self:
    return User(name=s)

fn round_trip<T>(x: T) -> T where T: Encodable:
  let encoded = x.encode()
  return T.decode(encoded)

fn main():
  let u = User(name="Ada")
  let result = round_trip<User>(u)
  print(result.name)
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    let _ = lsp.wait_diagnostics(&main_uri);

    let (line, col) = line_col_of(src, "round_trip<User>(u)");
    let help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(&main_uri, line, col + "round_trip<User>(".len()),
    );
    let label = signature_label(&help);
    assert!(
        label.contains("<T>"),
        "signature label should include type param <T>: {label}"
    );
    assert!(
        label.contains("where T: Encodable"),
        "signature label should include where clause: {label}"
    );
    assert!(
        label.contains("fn round_trip"),
        "signature label should include fn name: {label}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// Hover on a constrained type parameter inside a generic function shows the constraint.
#[test]
fn lsp_hover_on_constrained_type_param_shows_constraint() {
    let dir = temp_project_dir("fuse_lsp_generics_hover");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let src = r#"interface Encodable:
  fn encode() -> String
  fn decode(s: String) -> Self

fn round_trip<T>(x: T) -> T where T: Encodable:
  let encoded = x.encode()
  return T.decode(encoded)
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    let _ = lsp.wait_diagnostics(&main_uri);

    // Hover on the `T` in the parameter `x: T`
    let (line, col) = line_col_of(src, "x: T) -> T where");
    let hover = lsp.request(
        "textDocument/hover",
        position_params(&main_uri, line, col + "x: ".len()),
    );
    let hover_text = json::encode(&hover);
    assert!(
        hover_text.contains("Encodable"),
        "hover on constrained type param T should mention Encodable constraint: {hover_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// Completion inside a generic function body offers the in-scope type param.
#[test]
fn lsp_completion_offers_type_params_inside_generic_body() {
    let dir = temp_project_dir("fuse_lsp_generics_completion");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    // The `T` appears as a parameter type and should be in scope inside the function body.
    let src = r#"interface Encodable:
  fn encode() -> String
  fn decode(s: String) -> Self

fn process<T>(x: T) -> T where T: Encodable:
  let encoded = x.encode()
  return T
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    let _ = lsp.wait_diagnostics(&main_uri);

    // Request completion inside the function body — position just after `return `
    let (line, col) = line_col_of(src, "return T");
    let completion = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "return ".len()),
    );
    let labels = completion_labels(&completion);
    assert!(
        labels.iter().any(|l| l == "T"),
        "completion inside generic body should offer type param T; got: {labels:?}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// Impl stub completion renders generic interface member signatures (with type params and where).
#[test]
fn lsp_impl_stub_completion_renders_generic_interface_members() {
    let dir = temp_project_dir("fuse_lsp_generics_stub");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let src = r#"interface Transformer:
  fn transform<T>(value: T) -> T where T: Transformer
  fn identity() -> Self

type Wrapper:
  inner: String

impl Transformer for Wrapper:
  fn identity() -> Self:
    return Wrapper(inner="")
  # stub
"#;
    let main_path = dir.join("main.fuse");
    let main_uri = path_to_uri(&main_path);
    write_project_file(&main_path, src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, src, 1);
    let _ = lsp.wait_diagnostics(&main_uri);

    let (line, col) = line_col_of(src, "impl Transformer for Wrapper:");
    let completion = lsp.request(
        "textDocument/completion",
        completion_params(
            &main_uri,
            line,
            col + "impl Transformer for Wrapper:".len(),
        ),
    );
    let completion_text = json::encode(&completion);
    assert!(
        completion_text.contains("\"label\":\"transform\""),
        "missing impl stub completion label for generic member: {completion_text}"
    );
    assert!(
        completion_text.contains("<T>"),
        "impl stub completion should render generic type param <T>: {completion_text}"
    );
    assert!(
        completion_text.contains("where T: Transformer"),
        "impl stub completion should render where clause: {completion_text}"
    );
    assert!(
        completion_text.contains("TODO: implement transform"),
        "impl stub completion should include TODO body: {completion_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

/// Impl skeleton code action renders generic interface members correctly.
#[test]
fn lsp_code_action_generates_impl_skeleton_with_generic_members() {
    let dir = temp_project_dir("fuse_lsp_generics_skeleton");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let src = r#"interface Codec:
  fn encode<T>(value: T) -> String where T: Codec
  fn decode(text: String) -> Self

type Record:
  data: String
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
        actions_text.contains("Generate impl Codec for Record"),
        "missing impl skeleton code action: {actions_text}"
    );
    assert!(
        actions_text.contains("impl Codec for Record:"),
        "skeleton should include impl header: {actions_text}"
    );
    assert!(
        actions_text.contains("<T>"),
        "skeleton should render generic type param <T> for encode: {actions_text}"
    );
    assert!(
        actions_text.contains("where T: Codec"),
        "skeleton should render where clause for encode: {actions_text}"
    );
    assert!(
        actions_text.contains("fn decode(text: String) -> Self:"),
        "skeleton should render non-generic decode member: {actions_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
