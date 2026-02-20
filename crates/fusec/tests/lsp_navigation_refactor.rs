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

#[test]
fn lsp_prepare_rename_navigation_and_call_hierarchy_across_modules() {
    let dir = temp_project_dir("fuse_lsp_navigation_refactor");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let util_src = r#"fn greet(name: String) -> String:
  return name
"#;
    let api_src = r#"import util from "./util"

fn endpoint(name: String) -> String:
  return util.greet(name)
"#;
    let main_src = r#"import util from "./util"
import { endpoint } from "./api"

fn local_id(value: String) -> String:
  return value

fn main():
  let a = local_id(util.greet("A"))
  let b = endpoint("B")
  print(a)
  print(b)
"#;

    let util_path = dir.join("util.fuse");
    let api_path = dir.join("api.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, util_src);
    write_project_file(&api_path, api_src);
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let api_uri = path_to_uri(&api_path);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&util_uri, util_src, 1);
    lsp.open_document(&api_uri, api_src, 1);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&api_uri).is_empty());
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (main_greet_line, main_greet_col) = line_col_of(main_src, "util.greet(\"A\")");
    let definition = lsp.request(
        "textDocument/definition",
        position_params(
            &main_uri,
            main_greet_line,
            main_greet_col + "util.".len() + 1,
        ),
    );
    let definition_text = json::encode(&definition);
    assert!(
        definition_text.contains(&util_uri),
        "definition should resolve to util module: {definition_text}"
    );

    let (util_greet_line, util_greet_col) = line_col_of(util_src, "fn greet");
    let mut refs_params = match position_params(&util_uri, util_greet_line, util_greet_col + 3) {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    let mut refs_context = BTreeMap::new();
    refs_context.insert("includeDeclaration".to_string(), JsonValue::Bool(true));
    refs_params.insert("context".to_string(), JsonValue::Object(refs_context));
    let refs = lsp.request("textDocument/references", JsonValue::Object(refs_params));
    let refs_text = json::encode(&refs);
    assert!(
        refs_text.contains(&util_uri),
        "references should include declaration for the requested definition: {refs_text}"
    );

    let mut refs_from_call_params = match position_params(
        &main_uri,
        main_greet_line,
        main_greet_col + "util.".len() + 1,
    ) {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    let mut refs_context = BTreeMap::new();
    refs_context.insert("includeDeclaration".to_string(), JsonValue::Bool(true));
    refs_from_call_params.insert("context".to_string(), JsonValue::Object(refs_context));
    let refs_from_call = lsp.request(
        "textDocument/references",
        JsonValue::Object(refs_from_call_params),
    );
    let refs_from_call_text = json::encode(&refs_from_call);
    assert!(
        refs_from_call_text.contains(&main_uri),
        "references from callsite should include local callsite entries: {refs_from_call_text}"
    );

    let prepare = lsp.request(
        "textDocument/prepareRename",
        position_params(&util_uri, util_greet_line, util_greet_col + 3),
    );
    let prepare_text = json::encode(&prepare);
    assert!(
        prepare_text.contains("\"placeholder\":\"greet\"") && prepare_text.contains("\"range\""),
        "prepareRename should return rename range/placeholder: {prepare_text}"
    );

    let (print_line, print_col) = line_col_of(main_src, "print(a)");
    let prepare_builtin = lsp.request(
        "textDocument/prepareRename",
        position_params(&main_uri, print_line, print_col + 1),
    );
    assert_eq!(
        prepare_builtin,
        JsonValue::Null,
        "prepareRename should reject builtin targets"
    );

    let (return_line, return_col) = line_col_of(api_src, "return util.greet(name)");
    let prepare_keyword = lsp.request(
        "textDocument/prepareRename",
        position_params(&api_uri, return_line, return_col + 1),
    );
    assert_eq!(
        prepare_keyword,
        JsonValue::Null,
        "prepareRename should reject keyword targets"
    );

    let mut rename_invalid = match position_params(&util_uri, util_greet_line, util_greet_col + 3) {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    rename_invalid.insert("newName".to_string(), JsonValue::String("if".to_string()));
    let invalid_rename = lsp.request("textDocument/rename", JsonValue::Object(rename_invalid));
    assert_eq!(
        invalid_rename,
        JsonValue::Null,
        "rename should reject keyword new names"
    );

    let (local_id_line, local_id_col) = line_col_of(main_src, "fn local_id");
    let prepare_hierarchy = lsp.request(
        "textDocument/prepareCallHierarchy",
        position_params(&main_uri, local_id_line, local_id_col + 3),
    );
    let local_id_item = match prepare_hierarchy {
        JsonValue::Array(items) if !items.is_empty() => items[0].clone(),
        other => panic!("prepareCallHierarchy returned unexpected payload: {other:?}"),
    };
    let mut incoming_params = BTreeMap::new();
    incoming_params.insert("item".to_string(), local_id_item.clone());
    let incoming = lsp.request(
        "callHierarchy/incomingCalls",
        JsonValue::Object(incoming_params),
    );
    let incoming_text = json::encode(&incoming);
    assert!(
        incoming_text.contains("\"name\":\"main\""),
        "incoming call hierarchy should include local caller: {incoming_text}"
    );

    let (main_line, main_col) = line_col_of(main_src, "fn main");
    let prepare_endpoint = lsp.request(
        "textDocument/prepareCallHierarchy",
        position_params(&main_uri, main_line, main_col + 3),
    );
    let main_item = match prepare_endpoint {
        JsonValue::Array(items) if !items.is_empty() => items[0].clone(),
        other => panic!("prepareCallHierarchy returned unexpected payload: {other:?}"),
    };
    let mut outgoing_params = BTreeMap::new();
    outgoing_params.insert("item".to_string(), main_item);
    let outgoing = lsp.request(
        "callHierarchy/outgoingCalls",
        JsonValue::Object(outgoing_params),
    );
    let outgoing_text = json::encode(&outgoing);
    assert!(
        outgoing_text.contains("\"name\":\"local_id\""),
        "outgoing call hierarchy should include local call target: {outgoing_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
