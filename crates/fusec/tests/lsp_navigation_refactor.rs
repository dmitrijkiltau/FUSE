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
    let alias_definition = lsp.request(
        "textDocument/definition",
        position_params(&main_uri, main_greet_line, main_greet_col + 1),
    );
    let alias_definition_text = json::encode(&alias_definition);
    assert!(
        alias_definition_text.contains(&util_uri),
        "definition on module alias receiver should resolve to util module: {alias_definition_text}"
    );

    let alias_hover = lsp.request(
        "textDocument/hover",
        position_params(&main_uri, main_greet_line, main_greet_col + 1),
    );
    let alias_hover_text = json::encode(&alias_hover);
    assert!(
        alias_hover_text.contains("\"kind\":\"markdown\""),
        "hover on module alias receiver should return markdown contents: {alias_hover_text}"
    );
    assert!(
        alias_hover_text.contains("Module") && alias_hover_text.contains("util"),
        "hover on module alias receiver should describe the local module binding: {alias_hover_text}"
    );
    assert!(
        alias_hover_text.contains(&util_uri) && alias_hover_text.contains("greet"),
        "hover on module alias receiver should describe the target module and exports: {alias_hover_text}"
    );

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

    let mut alias_refs_params = match position_params(&main_uri, main_greet_line, main_greet_col + 1) {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    let mut alias_refs_context = BTreeMap::new();
    alias_refs_context.insert("includeDeclaration".to_string(), JsonValue::Bool(true));
    alias_refs_params.insert("context".to_string(), JsonValue::Object(alias_refs_context));
    let alias_refs = lsp.request("textDocument/references", JsonValue::Object(alias_refs_params));
    let alias_refs_text = json::encode(&alias_refs);
    assert!(
        alias_refs_text.contains(&main_uri),
        "references on module alias receiver should include the local binding and use site: {alias_refs_text}"
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

    let alias_prepare = lsp.request(
        "textDocument/prepareRename",
        position_params(&main_uri, main_greet_line, main_greet_col + 1),
    );
    let alias_prepare_text = json::encode(&alias_prepare);
    assert!(
        alias_prepare_text.contains("\"placeholder\":\"util\"")
            && alias_prepare_text.contains("\"range\""),
        "prepareRename should allow module alias receiver tokens: {alias_prepare_text}"
    );

    let mut alias_rename = match position_params(&main_uri, main_greet_line, main_greet_col + 1) {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    alias_rename.insert(
        "newName".to_string(),
        JsonValue::String("helpers".to_string()),
    );
    let alias_rename_result = lsp.request("textDocument/rename", JsonValue::Object(alias_rename));
    let alias_rename_text = json::encode(&alias_rename_result);
    let JsonValue::Object(alias_rename_root) = &alias_rename_result else {
        panic!("module alias rename should return workspace edits: {alias_rename_text}");
    };
    let Some(JsonValue::Object(alias_changes)) = alias_rename_root.get("changes") else {
        panic!("module alias rename should return a workspace change map: {alias_rename_text}");
    };
    let Some(JsonValue::Array(alias_main_edits)) = alias_changes.get(&main_uri) else {
        panic!("module alias rename should edit the current module: {alias_rename_text}");
    };
    assert_eq!(
        alias_main_edits.len(),
        2,
        "module alias rename should edit both the import binding and the receiver use site: {alias_rename_text}"
    );
    let mut alias_positions = Vec::new();
    for edit in alias_main_edits {
        let JsonValue::Object(edit_obj) = edit else {
            continue;
        };
        let Some(JsonValue::String(new_text)) = edit_obj.get("newText") else {
            continue;
        };
        assert_eq!(new_text, "helpers", "module alias rename should use requested name");
        let Some(JsonValue::Object(range_obj)) = edit_obj.get("range") else {
            continue;
        };
        let Some(JsonValue::Object(start_obj)) = range_obj.get("start") else {
            continue;
        };
        let Some(JsonValue::Number(line)) = start_obj.get("line") else {
            continue;
        };
        let Some(JsonValue::Number(character)) = start_obj.get("character") else {
            continue;
        };
        alias_positions.push((*line as usize, *character as usize));
    }
    assert!(
        alias_positions.contains(&(0, 7)) && alias_positions.contains(&(7, 19)),
        "module alias rename should cover import binding and receiver use site: {alias_rename_text}"
    );

    let (endpoint_call_line, endpoint_call_col) = line_col_of(main_src, "endpoint(\"B\")");
    let mut rename_endpoint = match position_params(&main_uri, endpoint_call_line, endpoint_call_col + 1)
    {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    rename_endpoint.insert(
        "newName".to_string(),
        JsonValue::String("dispatch".to_string()),
    );
    let endpoint_rename = lsp.request("textDocument/rename", JsonValue::Object(rename_endpoint));
    let endpoint_rename_text = json::encode(&endpoint_rename);
    assert!(
        endpoint_rename_text.contains(&api_uri) && endpoint_rename_text.contains(&main_uri),
        "rename should edit definition + importer module: {endpoint_rename_text}"
    );
    assert!(
        endpoint_rename_text.contains("\"newText\":\"dispatch\""),
        "rename should use the requested symbol name: {endpoint_rename_text}"
    );
    let JsonValue::Object(endpoint_rename_root) = &endpoint_rename else {
        panic!("rename should return workspace edits: {endpoint_rename_text}");
    };
    let Some(JsonValue::Object(endpoint_changes)) = endpoint_rename_root.get("changes") else {
        panic!("rename should return workspace change map: {endpoint_rename_text}");
    };
    let Some(JsonValue::Array(main_edits)) = endpoint_changes.get(&main_uri) else {
        panic!("rename should include main module edits: {endpoint_rename_text}");
    };
    assert_eq!(
        main_edits.len(),
        2,
        "rename should rewrite both the named import and the callsite in main: {endpoint_rename_text}"
    );
    let mut main_edit_positions = Vec::new();
    for edit in main_edits {
        let JsonValue::Object(edit_obj) = edit else {
            continue;
        };
        let Some(JsonValue::Object(range_obj)) = edit_obj.get("range") else {
            continue;
        };
        let Some(JsonValue::Object(start_obj)) = range_obj.get("start") else {
            continue;
        };
        let Some(JsonValue::Number(line)) = start_obj.get("line") else {
            continue;
        };
        let Some(JsonValue::Number(character)) = start_obj.get("character") else {
            continue;
        };
        main_edit_positions.push((*line as usize, *character as usize));
    }
    assert!(
        main_edit_positions.contains(&(1, 9)) && main_edit_positions.contains(&(8, 10)),
        "rename should cover both the named import and the callsite in main: {endpoint_rename_text}"
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

#[test]
fn lsp_rename_is_stable_across_root_and_dep_imports() {
    let dir = temp_project_dir("fuse_lsp_rename_root_dep");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n\n[dependencies]\nAuth = { path = \"./deps/auth\" }\n",
    );

    let main_src = r#"import Core from "root:lib/core"
import Auth from "dep:Auth/lib"

fn main():
  let a = Core.plus_one(1)
  let b = Auth.plus_one(2)
  print(a + b)
"#;
    let core_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;
    let dep_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let main_path = dir.join("main.fuse");
    let core_path = dir.join("lib").join("core.fuse");
    let dep_path = dir.join("deps").join("auth").join("lib.fuse");
    write_project_file(&main_path, main_src);
    write_project_file(&core_path, core_src);
    write_project_file(&dep_path, dep_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let core_uri = path_to_uri(&core_path);
    let dep_uri = path_to_uri(&dep_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&core_uri, core_src, 1);
    lsp.open_document(&dep_uri, dep_src, 1);
    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&core_uri).is_empty());
    assert!(lsp.wait_diagnostics(&dep_uri).is_empty());
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (root_call_line, root_call_col) = line_col_of(main_src, "Core.plus_one(1)");
    let root_definition = lsp.request(
        "textDocument/definition",
        position_params(&main_uri, root_call_line, root_call_col + "Core.".len() + 1),
    );
    let root_definition_text = json::encode(&root_definition);
    assert!(
        root_definition_text.contains(&core_uri),
        "root import definition should resolve to root module: {root_definition_text}"
    );

    let (core_def_line, core_def_col) = line_col_of(core_src, "fn plus_one");
    let root_prepare = lsp.request(
        "textDocument/prepareRename",
        position_params(&core_uri, core_def_line, core_def_col + 3),
    );
    let root_prepare_text = json::encode(&root_prepare);
    assert!(
        root_prepare_text.contains("\"placeholder\":\"plus_one\"")
            && root_prepare_text.contains("\"range\""),
        "prepareRename should allow root: export target: {root_prepare_text}"
    );

    let mut root_rename_params =
        match position_params(&main_uri, root_call_line, root_call_col + "Core.".len() + 1) {
            JsonValue::Object(params) => params,
            _ => unreachable!(),
        };
    root_rename_params.insert(
        "newName".to_string(),
        JsonValue::String("plus_root".to_string()),
    );
    let root_rename = lsp.request("textDocument/rename", JsonValue::Object(root_rename_params));
    let root_rename_text = json::encode(&root_rename);
    assert!(
        root_rename_text.contains(&main_uri) && root_rename_text.contains(&core_uri),
        "rename from root import call should edit caller + root module: {root_rename_text}"
    );
    assert!(
        !root_rename_text.contains(&dep_uri),
        "rename from root import call must not edit dep module: {root_rename_text}"
    );

    let (dep_call_line, dep_call_col) = line_col_of(main_src, "Auth.plus_one(2)");
    let dep_definition = lsp.request(
        "textDocument/definition",
        position_params(&main_uri, dep_call_line, dep_call_col + "Auth.".len() + 1),
    );
    let dep_definition_text = json::encode(&dep_definition);
    assert!(
        dep_definition_text.contains(&dep_uri),
        "dep import definition should resolve to dependency module: {dep_definition_text}"
    );

    let (dep_def_line, dep_def_col) = line_col_of(dep_src, "fn plus_one");
    let dep_prepare = lsp.request(
        "textDocument/prepareRename",
        position_params(&dep_uri, dep_def_line, dep_def_col + 3),
    );
    let dep_prepare_text = json::encode(&dep_prepare);
    assert!(
        dep_prepare_text.contains("\"placeholder\":\"plus_one\"")
            && dep_prepare_text.contains("\"range\""),
        "prepareRename should allow dep: export target: {dep_prepare_text}"
    );

    let mut dep_rename_params =
        match position_params(&main_uri, dep_call_line, dep_call_col + "Auth.".len() + 1) {
            JsonValue::Object(params) => params,
            _ => unreachable!(),
        };
    dep_rename_params.insert(
        "newName".to_string(),
        JsonValue::String("plus_dep".to_string()),
    );
    let dep_rename = lsp.request("textDocument/rename", JsonValue::Object(dep_rename_params));
    let dep_rename_text = json::encode(&dep_rename);
    assert!(
        dep_rename_text.contains(&main_uri) && dep_rename_text.contains(&dep_uri),
        "rename from dep import call should edit caller + dep module: {dep_rename_text}"
    );
    assert!(
        !dep_rename_text.contains(&core_uri),
        "rename from dep import call must not edit unrelated root module: {dep_rename_text}"
    );

    let (root_import_line, root_import_col) = line_col_of(main_src, "\"root:lib/core\"");
    let prepare_import_path = lsp.request(
        "textDocument/prepareRename",
        position_params(&main_uri, root_import_line, root_import_col + 2),
    );
    assert_eq!(
        prepare_import_path,
        JsonValue::Null,
        "prepareRename should reject import path strings"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_definition_navigates_asset_import_paths() {
    let dir = temp_project_dir("fuse_lsp_asset_path_definition");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n\n[dependencies]\nFixtures = { path = \"./deps/fixtures\" }\n",
    );
    write_project_file(&dir.join("README.md"), "local docs\n");
    write_project_file(&dir.join("content").join("policy.md"), "root policy\n");
    write_project_file(
        &dir.join("deps").join("fixtures").join("fuse.toml"),
        "[package]\nentry = \"lib.fuse\"\n",
    );
    write_project_file(
        &dir.join("deps").join("fixtures").join("lib.fuse"),
        "fn unused() -> Int:\n  return 0\n",
    );
    write_project_file(
        &dir.join("deps").join("fixtures").join("auth").join("login.json"),
        "{\"token\":\"abc123\"}",
    );

    let main_src = r#"import Docs from "./README.md"
import Policy from "root:content/policy.md"
import Auth from "dep:Fixtures/auth/login.json"

fn main():
  print(Docs)
  print(Policy)
  print(json.encode(Auth))
"#;
    let main_path = dir.join("main.fuse");
    write_project_file(&main_path, main_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let docs_uri = path_to_uri(&dir.join("README.md"));
    let policy_uri = path_to_uri(&dir.join("content").join("policy.md"));
    let auth_uri = path_to_uri(&dir.join("deps").join("fixtures").join("auth").join("login.json"));
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());

    let (docs_line, docs_col) = line_col_of(main_src, "\"./README.md\"");
    let docs_def = lsp.request(
        "textDocument/definition",
        position_params(&main_uri, docs_line, docs_col + 2),
    );
    let docs_def_text = json::encode(&docs_def);
    assert!(
        docs_def_text.contains(&docs_uri),
        "local asset path should navigate to markdown file: {docs_def_text}"
    );

    let (policy_line, policy_col) = line_col_of(main_src, "\"root:content/policy.md\"");
    let policy_def = lsp.request(
        "textDocument/definition",
        position_params(&main_uri, policy_line, policy_col + 2),
    );
    let policy_def_text = json::encode(&policy_def);
    assert!(
        policy_def_text.contains(&policy_uri),
        "root asset path should navigate to markdown file: {policy_def_text}"
    );

    let (auth_line, auth_col) = line_col_of(main_src, "\"dep:Fixtures/auth/login.json\"");
    let auth_def = lsp.request(
        "textDocument/definition",
        position_params(&main_uri, auth_line, auth_col + 2),
    );
    let auth_def_text = json::encode(&auth_def);
    assert!(
        auth_def_text.contains(&auth_uri),
        "dep asset path should navigate to json file: {auth_def_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
