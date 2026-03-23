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

fn signature_help_params(uri: &str, line: usize, character: usize) -> JsonValue {
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

fn signature_summary(result: &JsonValue) -> (String, Vec<String>, usize) {
    let JsonValue::Object(root) = result else {
        panic!(
            "signature help should be object, got {}",
            json::encode(result)
        );
    };
    let active_param = match root.get("activeParameter") {
        Some(JsonValue::Number(value)) if *value >= 0.0 => *value as usize,
        _ => 0,
    };
    let Some(JsonValue::Array(signatures)) = root.get("signatures") else {
        panic!("missing signatures payload: {}", json::encode(result));
    };
    let Some(JsonValue::Object(signature)) = signatures.first() else {
        panic!("missing first signature: {}", json::encode(result));
    };
    let label = match signature.get("label") {
        Some(JsonValue::String(label)) => label.clone(),
        _ => panic!("missing signature label: {}", json::encode(result)),
    };
    let mut params = Vec::new();
    if let Some(JsonValue::Array(items)) = signature.get("parameters") {
        for item in items {
            if let JsonValue::Object(param) = item {
                if let Some(JsonValue::String(label)) = param.get("label") {
                    params.push(label.clone());
                }
            }
        }
    }
    (label, params, active_param)
}

#[test]
fn lsp_signature_help_local_imported_and_builtin_calls() {
    let dir = temp_project_dir("fuse_lsp_signature_help");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let util_src = r#"type Person:
  name: String

fn greet(user: Person, times: Int) -> String:
  return "${user.name} x ${times}"
"#;
    let main_src = r#"import { Person, greet } from "./util"

fn local_join(left: String, right: String) -> String:
  return "${left}-${right}"

fn local_tail(prefix: String, suffix: String) -> String:
  return "${prefix}${suffix}"

fn main():
  let user = Person(name="Ada")
  let local = local_join("a", "b")
  let nested = local_join(greet(user, 2), local_tail("x", "y"))
  let remote = greet(user, 2)
  print(nested)
  print(remote)
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

    let (local_line, local_col) = line_col_of(main_src, "local_join(\"a\", \"b\")");
    let local_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(
            &main_uri,
            local_line,
            local_col + "local_join(\"a\", \"".len(),
        ),
    );
    let (local_label, local_params, local_active) = signature_summary(&local_help);
    assert!(
        local_label.contains("fn local_join(left: String, right: String) -> String"),
        "unexpected local signature label: {local_label}"
    );
    assert_eq!(
        local_active, 1,
        "local call active parameter should be second"
    );
    assert_eq!(
        local_params,
        vec!["left: String".to_string(), "right: String".to_string()],
        "unexpected local signature params"
    );

    let (nested_outer_line, nested_outer_col) = line_col_of(
        main_src,
        "local_join(greet(user, 2), local_tail(\"x\", \"y\"))",
    );
    let nested_outer_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(
            &main_uri,
            nested_outer_line,
            nested_outer_col + "local_join(greet(user, 2),".len(),
        ),
    );
    let (nested_outer_label, nested_outer_params, nested_outer_active) =
        signature_summary(&nested_outer_help);
    assert!(
        nested_outer_label.contains("fn local_join(left: String, right: String) -> String"),
        "unexpected outer nested signature label: {nested_outer_label}"
    );
    assert_eq!(
        nested_outer_active, 1,
        "outer nested call active parameter should be second"
    );
    assert_eq!(
        nested_outer_params,
        vec!["left: String".to_string(), "right: String".to_string()],
        "unexpected outer nested signature params"
    );

    let (nested_inner_line, nested_inner_col) =
        line_col_of(main_src, "greet(user, 2), local_tail(\"x\", \"y\")");
    let nested_inner_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(
            &main_uri,
            nested_inner_line,
            nested_inner_col + "greet(user, ".len(),
        ),
    );
    let (nested_inner_label, nested_inner_params, nested_inner_active) =
        signature_summary(&nested_inner_help);
    assert!(
        nested_inner_label.contains("fn greet(user: Person, times: Int) -> String"),
        "unexpected inner nested signature label: {nested_inner_label}"
    );
    assert_eq!(
        nested_inner_active, 1,
        "inner nested call active parameter should be second"
    );
    assert_eq!(
        nested_inner_params,
        vec!["user: Person".to_string(), "times: Int".to_string()],
        "unexpected inner nested signature params"
    );

    let (remote_line, remote_col) = line_col_of(main_src, "let remote = greet(user, 2)");
    let remote_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(
            &main_uri,
            remote_line,
            remote_col + "let remote = greet(user, ".len(),
        ),
    );
    let (remote_label, remote_params, remote_active) = signature_summary(&remote_help);
    assert!(
        remote_label.contains("fn greet(user: Person, times: Int) -> String"),
        "unexpected imported signature label: {remote_label}"
    );
    assert_eq!(
        remote_active, 1,
        "imported call active parameter should be second"
    );
    assert_eq!(
        remote_params,
        vec!["user: Person".to_string(), "times: Int".to_string()],
        "unexpected imported signature params"
    );

    let (print_line, print_col) = line_col_of(main_src, "print(remote)");
    let print_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(&main_uri, print_line, print_col + "print(".len()),
    );
    let (print_label, print_params, print_active) = signature_summary(&print_help);
    assert!(
        print_label.contains("fn print(value)"),
        "unexpected builtin signature label: {print_label}"
    );
    assert_eq!(print_active, 0, "builtin print should use first parameter");
    assert_eq!(
        print_params,
        vec!["value".to_string()],
        "unexpected builtin signature params"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_signature_help_module_alias_member_call() {
    let dir = temp_project_dir("fuse_lsp_signature_help_module_alias");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

        let util_src = r#"fn greet(name: String, times: Int) -> String:
    return "${name} x ${times}"
"#;
        let main_src = r#"import util from "./util"

fn main():
    let out = util.greet("Ada", 2)
    print(out)
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

    let (line, col) = line_col_of(main_src, "util.greet(\"Ada\", 2)");
    let help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(&main_uri, line, col + "util.greet(\"Ada\", ".len()),
    );
    let (label, params, active) = signature_summary(&help);
    assert!(
        label.contains("fn greet(name: String, times: Int) -> String"),
        "unexpected module-alias signature label: {label}"
    );
    assert_eq!(
        active, 1,
        "module-alias call active parameter should be second"
    );
    assert_eq!(
        params,
        vec!["name: String".to_string(), "times: Int".to_string()],
        "unexpected module-alias signature params"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_signature_help_http_builtin_member_call() {
    let dir = temp_project_dir("fuse_lsp_signature_help_http");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let main_src = r#"requires network

fn main():
  let _ = http.post("http://127.0.0.1:8080/submit", "payload", {}, 1000)
"#;
    let main_path = dir.join("main.fuse");
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, main_src, 1);
    let _ = lsp.wait_diagnostics(&main_uri);

    let (line, col) = line_col_of(main_src, "http.post(\"http://127.0.0.1:8080/submit\", \"payload\", {}, 1000)");
    let help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(
            &main_uri,
            line,
            col + "http.post(\"http://127.0.0.1:8080/submit\", \"payload\", ".len(),
        ),
    );
    let (label, params, active) = signature_summary(&help);
    assert!(
        label.contains("fn http.post(url: String, body: String, headers?: Map<String, String>, timeout_ms?: Int) -> http.response!http.error"),
        "unexpected http signature label: {label}"
    );
    assert_eq!(active, 2, "http.post active parameter should be headers");
    assert_eq!(
        params,
        vec![
            "url: String".to_string(),
            "body: String".to_string(),
            "headers: Map<String, String>".to_string(),
            "timeout_ms: Int".to_string(),
        ],
        "unexpected http signature params"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}


#[test]
fn lsp_signature_help_prefers_concrete_impl_methods() {
    let dir = temp_project_dir("fuse_lsp_signature_help_interface");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let main_src = r#"interface Codec:
  fn encode(format: String) -> String
  fn from_text(text: String) -> Self

type Note:
  text: String

impl Codec for Note:
  ## Encodes the note body.
  fn encode(format: String) -> String:
    return self.text

  ## Builds a note from text.
  fn from_text(text: String) -> Self:
    return Note(text=text)

fn main():
  let note = Note(text="hi")
  let encoded = note.encode("json")
  let decoded = Note.from_text("hello")
  print(encoded)
  print(decoded.body)
"#;
    let main_path = dir.join("main.fuse");
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, main_src, 1);
    let _ = lsp.wait_diagnostics(&main_uri);

    let (instance_line, instance_col) = line_col_of(main_src, "note.encode(\"json\")");
    let instance_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(
            &main_uri,
            instance_line,
            instance_col + "note.encode(\"".len(),
        ),
    );
    let (instance_label, instance_params, instance_active) = signature_summary(&instance_help);
    let instance_text = json::encode(&instance_help);
    assert!(
        instance_label.contains("fn encode(format: String) -> String"),
        "unexpected impl instance signature label: {instance_label}"
    );
    assert_eq!(instance_active, 0, "instance impl call should use first parameter");
    assert_eq!(
        instance_params,
        vec!["format: String".to_string()],
        "unexpected impl instance signature params"
    );
    assert!(
        instance_text.contains("Encodes the note body."),
        "instance impl signature help should include impl doc: {instance_text}"
    );

    let (assoc_line, assoc_col) = line_col_of(main_src, "Note.from_text(\"hello\")");
    let assoc_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(
            &main_uri,
            assoc_line,
            assoc_col + "Note.from_text(\"".len(),
        ),
    );
    let (assoc_label, assoc_params, assoc_active) = signature_summary(&assoc_help);
    let assoc_text = json::encode(&assoc_help);
    assert!(
        assoc_label.contains("fn from_text(text: String) -> Self"),
        "unexpected impl associated signature label: {assoc_label}"
    );
    assert_eq!(assoc_active, 0, "associated impl call should use first parameter");
    assert_eq!(
        assoc_params,
        vec!["text: String".to_string()],
        "unexpected impl associated signature params"
    );
    assert!(
        assoc_text.contains("Builds a note from text."),
        "associated impl signature help should include impl doc: {assoc_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
