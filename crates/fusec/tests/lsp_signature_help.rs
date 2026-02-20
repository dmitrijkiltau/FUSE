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

fn main():
  let user = Person(name="Ada")
  let local = local_join("a", "b")
  let remote = greet(user, 2)
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
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

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

    let (remote_line, remote_col) = line_col_of(main_src, "greet(user, 2)");
    let remote_help = lsp.request(
        "textDocument/signatureHelp",
        signature_help_params(&main_uri, remote_line, remote_col + "greet(user, ".len()),
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
