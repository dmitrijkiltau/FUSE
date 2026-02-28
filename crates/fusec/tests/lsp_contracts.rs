use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use fuse_rt::json::{self, JsonValue};

#[path = "support/lsp.rs"]
mod lsp;
use lsp::{
    LspClient, path_to_uri, semantic_rows, temp_project_dir, token_type_at, write_project_file,
};

struct WorkspaceFixture {
    dir: PathBuf,
    util_uri: String,
    main_uri: String,
    util_src: String,
    main_src: String,
}

fn create_workspace_fixture(prefix: &str) -> WorkspaceFixture {
    let dir = temp_project_dir(prefix);
    fs::create_dir_all(&dir).expect("create temp dir");

    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let util_src = r#"type Person:
  name: String

## Says hello repeatedly.
fn greet(user: Person, times: Int) -> String:
  return "${user.name} x ${times}"

fn helper(input: String) -> String:
  return input
"#
    .to_string();
    let main_src = r#"requires db

import { Person, greet } from "./util"

fn local_id(input: String) -> String:
  return input

fn call_greet(user: Person) -> String:
  let rendered = greet(user, 2)
  return local_id(rendered)

fn main():
  let user: Person = Person(name="Ada")
  let out = call_greet(user)
  let rows = db
    .from("notes")
    .select(["id"])
    .all()
  let _typed: List<Map<String, String>> = rows
  print(out)
"#
    .to_string();

    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, &util_src);
    write_project_file(&main_path, &main_src);

    WorkspaceFixture {
        dir,
        util_uri: path_to_uri(&util_path),
        main_uri: path_to_uri(&main_path),
        util_src,
        main_src,
    }
}

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
fn lsp_lifecycle_diagnostics_open_change_close() {
    let dir = temp_project_dir("fuse_lsp_lifecycle");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    let broken = "fn main(\n";
    let fixed = "fn main():\n  print(1)\n";
    let main_path = dir.join("main.fuse");
    write_project_file(&main_path, broken);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&main_uri, broken, 1);
    let open_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        !open_diags.is_empty(),
        "expected diagnostics on broken input"
    );

    lsp.change_document(&main_uri, fixed, 2);
    let changed_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        changed_diags.is_empty(),
        "expected diagnostics to clear after fix, got {}",
        json::encode(&JsonValue::Array(changed_diags))
    );

    lsp.close_document(&main_uri);
    let close_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        close_diags.is_empty(),
        "expected close to publish empty diagnostics"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_navigation_refactor_workspace_symbol_and_call_hierarchy() {
    let fixture = create_workspace_fixture("fuse_lsp_navigation");
    let root_uri = path_to_uri(&fixture.dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&fixture.util_uri, &fixture.util_src, 1);
    lsp.open_document(&fixture.main_uri, &fixture.main_src, 1);
    assert!(lsp.wait_diagnostics(&fixture.util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&fixture.main_uri).is_empty());

    let (call_line, call_col) = line_col_of(&fixture.main_src, "greet(user, 2)");
    let definition = lsp.request(
        "textDocument/definition",
        position_params(&fixture.main_uri, call_line, call_col + 1),
    );
    let definition_text = json::encode(&definition);
    assert!(
        definition_text.contains(&fixture.util_uri),
        "definition missing util location: {definition_text}"
    );

    let mut refs_params = match position_params(&fixture.main_uri, call_line, call_col + 1) {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    let mut refs_context = BTreeMap::new();
    refs_context.insert("includeDeclaration".to_string(), JsonValue::Bool(true));
    refs_params.insert("context".to_string(), JsonValue::Object(refs_context));
    let refs = lsp.request("textDocument/references", JsonValue::Object(refs_params));
    let refs_text = json::encode(&refs);
    assert!(
        refs_text.contains(&fixture.main_uri),
        "references missing callsite entries: {refs_text}"
    );

    let mut rename_params = match position_params(&fixture.main_uri, call_line, call_col + 1) {
        JsonValue::Object(params) => params,
        _ => unreachable!(),
    };
    rename_params.insert(
        "newName".to_string(),
        JsonValue::String("greetAgain".to_string()),
    );
    let rename = lsp.request("textDocument/rename", JsonValue::Object(rename_params));
    let rename_text = json::encode(&rename);
    assert!(
        rename_text.contains("greetAgain") && rename_text.contains(&fixture.main_uri),
        "rename missing expected edits: {rename_text}"
    );

    let mut symbol_params = BTreeMap::new();
    symbol_params.insert("query".to_string(), JsonValue::String("greet".to_string()));
    let symbols = lsp.request("workspace/symbol", JsonValue::Object(symbol_params));
    let symbols_text = json::encode(&symbols);
    assert!(
        symbols_text.contains("\"name\":\"greet\"")
            && symbols_text.contains("\"name\":\"call_greet\""),
        "workspace symbols missing greet entries: {symbols_text}"
    );

    let (caller_line, caller_col) = line_col_of(&fixture.main_src, "fn call_greet");
    let prepare_call_greet = lsp.request(
        "textDocument/prepareCallHierarchy",
        position_params(&fixture.main_uri, caller_line, caller_col + 3),
    );
    let call_greet_item = match prepare_call_greet {
        JsonValue::Array(items) if !items.is_empty() => items[0].clone(),
        other => panic!("prepareCallHierarchy returned unexpected payload: {other:?}"),
    };
    let mut incoming_params = BTreeMap::new();
    incoming_params.insert("item".to_string(), call_greet_item.clone());
    let incoming = lsp.request(
        "callHierarchy/incomingCalls",
        JsonValue::Object(incoming_params),
    );
    let incoming_text = json::encode(&incoming);
    assert!(
        incoming_text.contains("\"name\":\"main\""),
        "incoming call hierarchy missing main caller: {incoming_text}"
    );

    let mut outgoing_params = BTreeMap::new();
    outgoing_params.insert("item".to_string(), call_greet_item);
    let outgoing = lsp.request(
        "callHierarchy/outgoingCalls",
        JsonValue::Object(outgoing_params),
    );
    let outgoing_text = json::encode(&outgoing);
    assert!(
        outgoing_text.contains("\"name\":\"local_id\"")
            && outgoing_text.contains(&fixture.main_uri),
        "outgoing call hierarchy missing local target: {outgoing_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(fixture.dir);
}

#[test]
fn lsp_completion_symbols_and_member_methods() {
    let fixture = create_workspace_fixture("fuse_lsp_completion");
    let root_uri = path_to_uri(&fixture.dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&fixture.util_uri, &fixture.util_src, 1);
    lsp.open_document(&fixture.main_uri, &fixture.main_src, 1);
    assert!(lsp.wait_diagnostics(&fixture.util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&fixture.main_uri).is_empty());

    let (call_line, call_col) = line_col_of(&fixture.main_src, "greet(user, 2)");
    let completion = lsp.request(
        "textDocument/completion",
        position_params(&fixture.main_uri, call_line, call_col + 2),
    );
    let completion_text = json::encode(&completion);
    assert!(
        completion_text.contains("\"label\":\"greet\""),
        "completion missing greet symbol: {completion_text}"
    );

    let (member_line, member_col) = line_col_of(&fixture.main_src, ".from(\"notes\")");
    let member_completion = lsp.request(
        "textDocument/completion",
        position_params(&fixture.main_uri, member_line, member_col + 3),
    );
    let member_text = json::encode(&member_completion);
    assert!(
        member_text.contains("\"label\":\"from\"") && member_text.contains("db builtin"),
        "member completion missing db methods: {member_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(fixture.dir);
}

#[test]
fn lsp_semantic_tokens_and_inlay_hints_contract() {
    let fixture = create_workspace_fixture("fuse_lsp_semantics");
    let root_uri = path_to_uri(&fixture.dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&fixture.util_uri, &fixture.util_src, 1);
    lsp.open_document(&fixture.main_uri, &fixture.main_src, 1);
    assert!(lsp.wait_diagnostics(&fixture.util_uri).is_empty());
    assert!(lsp.wait_diagnostics(&fixture.main_uri).is_empty());

    let mut inlay_doc = BTreeMap::new();
    inlay_doc.insert(
        "uri".to_string(),
        JsonValue::String(fixture.main_uri.clone()),
    );
    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(0.0));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert("line".to_string(), JsonValue::Number(80.0));
    range_end.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(range_start));
    range.insert("end".to_string(), JsonValue::Object(range_end));
    let mut inlay_params = BTreeMap::new();
    inlay_params.insert("textDocument".to_string(), JsonValue::Object(inlay_doc));
    inlay_params.insert("range".to_string(), JsonValue::Object(range));
    let inlays = lsp.request("textDocument/inlayHint", JsonValue::Object(inlay_params));
    let inlay_text = json::encode(&inlays);
    assert!(
        inlay_text.contains("user: ") && inlay_text.contains("times: "),
        "inlay hints missing parameter labels: {inlay_text}"
    );

    let mut sem_doc = BTreeMap::new();
    sem_doc.insert(
        "uri".to_string(),
        JsonValue::String(fixture.main_uri.clone()),
    );
    let mut sem_params = BTreeMap::new();
    sem_params.insert("textDocument".to_string(), JsonValue::Object(sem_doc));
    let sem = lsp.request(
        "textDocument/semanticTokens/full",
        JsonValue::Object(sem_params),
    );
    let sem_text = json::encode(&sem);
    assert!(
        sem_text.contains("\"data\"") && !sem_text.contains("\"data\":[]"),
        "semantic tokens unexpectedly empty: {sem_text}"
    );
    let rows = semantic_rows(&sem);

    let (import_person_line, import_person_col) = line_col_of(&fixture.main_src, "Person, greet");
    let (annot_person_line, annot_person_col) = line_col_of(&fixture.main_src, "user: Person");
    let (from_line, from_col) = line_col_of(&fixture.main_src, ".from(\"notes\")");
    let (select_line, select_col) = line_col_of(&fixture.main_src, ".select([\"id\"])");
    let import_person_ty = token_type_at(&rows, import_person_line, import_person_col)
        .expect("token type for imported Person");
    let annotate_person_ty = token_type_at(&rows, annot_person_line, annot_person_col + 6)
        .expect("token type for annotated Person");
    let from_ty =
        token_type_at(&rows, from_line, from_col + 1).expect("token type for from member");
    let select_ty =
        token_type_at(&rows, select_line, select_col + 1).expect("token type for select member");

    assert_eq!(
        import_person_ty, annotate_person_ty,
        "imported vs annotated type token mismatch"
    );
    assert_eq!(from_ty, select_ty, "db member token mismatch");

    lsp.shutdown();
    let _ = fs::remove_dir_all(fixture.dir);
}

#[test]
fn lsp_code_actions_and_formatting_contract() {
    let dir = temp_project_dir("fuse_lsp_actions");
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
    let main_missing_import = r#"import { helper } from "./util"

fn main():
  let err = NotFound()
  print(helper("Ada"))
  print(err)
"#;
    let main_unsorted_imports = r#"import { helper, greet } from "./util"

fn main():
  print(greet(helper("Ada"), 1))
"#;
    let main_unformatted = r#"fn main():
    let x=1
    print(x)
"#;

    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, util_src);
    write_project_file(&main_path, main_missing_import);
    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let main_uri = path_to_uri(&main_path);

    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&util_uri, util_src, 1);
    lsp.open_document(&main_uri, main_missing_import, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    let missing_import_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        !missing_import_diags.is_empty(),
        "expected unknown identifier diagnostics for missing import"
    );

    let mut action_doc = BTreeMap::new();
    action_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(0.0));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert("line".to_string(), JsonValue::Number(20.0));
    range_end.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(range_start));
    range.insert("end".to_string(), JsonValue::Object(range_end));
    let mut context = BTreeMap::new();
    context.insert(
        "diagnostics".to_string(),
        JsonValue::Array(missing_import_diags.clone()),
    );
    let mut code_action_params = BTreeMap::new();
    code_action_params.insert("textDocument".to_string(), JsonValue::Object(action_doc));
    code_action_params.insert("range".to_string(), JsonValue::Object(range));
    code_action_params.insert("context".to_string(), JsonValue::Object(context));
    let import_actions = lsp.request(
        "textDocument/codeAction",
        JsonValue::Object(code_action_params),
    );
    let import_actions_text = json::encode(&import_actions);
    assert!(
        import_actions_text.contains("Import NotFound from std.Error"),
        "missing import quickfix action: {import_actions_text}"
    );

    lsp.change_document(&main_uri, main_unsorted_imports, 2);
    let sorted_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        sorted_diags.is_empty(),
        "unexpected diagnostics in sortable import file"
    );

    let mut action_doc = BTreeMap::new();
    action_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(0.0));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert("line".to_string(), JsonValue::Number(20.0));
    range_end.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(range_start));
    range.insert("end".to_string(), JsonValue::Object(range_end));
    let mut context = BTreeMap::new();
    context.insert("diagnostics".to_string(), JsonValue::Array(Vec::new()));
    let mut code_action_params = BTreeMap::new();
    code_action_params.insert("textDocument".to_string(), JsonValue::Object(action_doc));
    code_action_params.insert("range".to_string(), JsonValue::Object(range));
    code_action_params.insert("context".to_string(), JsonValue::Object(context));
    let organize_actions = lsp.request(
        "textDocument/codeAction",
        JsonValue::Object(code_action_params),
    );
    let organize_actions_text = json::encode(&organize_actions);
    assert!(
        organize_actions_text.contains("Organize imports")
            && organize_actions_text.contains("greet, helper"),
        "missing organize imports action/edit: {organize_actions_text}"
    );

    lsp.change_document(&main_uri, main_unformatted, 3);
    let format_diags = lsp.wait_diagnostics(&main_uri);
    assert!(
        format_diags.is_empty(),
        "formatter scenario should parse without diagnostics"
    );
    let mut format_doc = BTreeMap::new();
    format_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut format_params = BTreeMap::new();
    format_params.insert("textDocument".to_string(), JsonValue::Object(format_doc));
    let formatting = lsp.request("textDocument/formatting", JsonValue::Object(format_params));
    let formatting_text = json::encode(&formatting);
    assert!(
        formatting_text.starts_with('['),
        "formatting response should be an edit array: {formatting_text}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
