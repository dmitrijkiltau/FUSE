use std::collections::BTreeMap;
use std::fs;

use fuse_rt::json::JsonValue;

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

fn workspace_builds(lsp: &mut LspClient) -> u64 {
    let result = lsp.request(
        "fuse/internalWorkspaceStats",
        JsonValue::Object(BTreeMap::new()),
    );
    let JsonValue::Object(stats) = result else {
        panic!("workspace stats must be an object");
    };
    let Some(JsonValue::Number(builds)) = stats.get("workspaceBuilds") else {
        panic!("workspace stats missing workspaceBuilds");
    };
    *builds as u64
}

#[test]
fn lsp_reuses_workspace_snapshot_for_diagnostics_and_index_requests() {
    let dir = temp_project_dir("fuse_lsp_workspace_incremental");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let mut util_src = String::new();
    for i in 0..300 {
        util_src.push_str(&format!(
            "fn f{i:04}(value: Int) -> Int:\n  return value + {i}\n\n"
        ));
    }
    let main_src = r#"import util from "./util"

fn main():
  let value = util.f0001(1)
  let total = util.f0002(value)
  print(total)
"#;
    let extra_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    let extra_path = dir.join("extra.fuse");
    write_project_file(&util_path, &util_src);
    write_project_file(&main_path, main_src);
    write_project_file(&extra_path, extra_src);

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let main_uri = path_to_uri(&main_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "open diagnostics should build once"
    );

    let (line, col) = line_col_of(main_src, "util.f0002");
    let completion = completion_params(&main_uri, line, col + "util.".len());

    let _ = lsp.request("textDocument/completion", completion.clone());
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "index request should reuse diagnostics workspace snapshot"
    );

    let main_src_v2 = r#"import util from "./util"

fn main():
  let value = util.f0001(1)
  let total = util.f0002(value)
  print(total)
  print(value)
"#;
    lsp.change_document(&main_uri, main_src_v2, 2);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "non-structural document revision should reuse workspace snapshot"
    );

    let _ = lsp.request("textDocument/completion", completion.clone());
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "post-change index request should reuse rebuilt snapshot"
    );

    lsp.open_document(&util_uri, &util_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "non-entry module diagnostics should reuse workspace snapshot"
    );

    let _ = lsp.request("textDocument/completion", completion);
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "main module completion should reuse manifest-rooted workspace built for util diagnostics"
    );

    let util_src_v2 = util_src.replacen("fn f0002", "fn g0002", 1);
    lsp.change_document(&util_uri, &util_src_v2, 2);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "export-shape changes should be relinked without full workspace rebuild"
    );

    let _ = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "util.".len()),
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "post-relink requests should reuse workspace snapshot"
    );

    let util_src_v3 = format!(
        "import extra from \"./extra\"\n{util_src_v2}\nfn probe_extra(value: Int) -> Int:\n  return extra.plus_one(value)\n"
    );
    lsp.change_document(&util_uri, &util_src_v3, 3);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        util_diags.is_empty(),
        "newly introduced existing module imports should relink incrementally"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "importing a previously unseen but existing module should avoid full reload fallback"
    );

    let _ = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "util.".len()),
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "post-relink requests should reuse workspace snapshot"
    );

    let util_src_v4 = format!("{util_src_v3}\nimport missing from \"./missing\"\n");
    lsp.change_document(&util_uri, &util_src_v4, 4);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        !util_diags.is_empty(),
        "unknown module import should still report diagnostics after fallback reload"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        2,
        "importing an unresolved module path should still fall back to a full reload"
    );

    lsp.close_document(&util_uri);
    assert_eq!(
        workspace_builds(&mut lsp),
        2,
        "close should invalidate cache without immediate rebuild"
    );
    let _ = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "util.".len()),
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        2,
        "post-close request should reuse incrementally relinked disk module state"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_incrementally_loads_new_dependency_modules_during_relink() {
    let dir = temp_project_dir("fuse_lsp_dep_incremental");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n\n[dependencies]\nAuth = { path = \"./deps/auth\" }\n",
    );

    let main_src = r#"import util from "./util"

fn main():
  print(util.local(1))
"#;
    let util_src = r#"fn local(value: Int) -> Int:
  return value + 1
"#;
    let util_src_v2 = r#"import Auth from "dep:Auth/lib"

fn local(value: Int) -> Int:
  return Auth.plus_one(value)
"#;
    let dep_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let main_path = dir.join("main.fuse");
    let util_path = dir.join("util.fuse");
    let dep_path = dir.join("deps").join("auth").join("lib.fuse");
    write_project_file(&main_path, main_src);
    write_project_file(&util_path, util_src);
    write_project_file(&dep_path, dep_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let util_uri = path_to_uri(&util_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.open_document(&util_uri, util_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.change_document(&util_uri, util_src_v2, 2);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        util_diags.is_empty(),
        "newly introduced dep imports should relink incrementally"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "new dependency import paths should not force full workspace rebuild"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_resolves_dep_imports_for_dependency_manifest_variants() {
    let dir = temp_project_dir("fuse_lsp_dep_manifest_variants");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n\n[dependencies]\nAuthString = \"./deps/auth-string\"\nAuthInline = { path = \"./deps/auth-inline\" }\n\n[dependencies.AuthTable]\npath = \"./deps/auth-table\"\n",
    );

    let main_src = r#"import util from "./util"

fn main():
  print(util.local(1))
"#;
    let util_src = r#"import AuthString from "dep:AuthString/lib"
import AuthInline from "dep:AuthInline/lib"
import AuthTable from "dep:AuthTable/lib"

fn local(value: Int) -> Int:
  let a = AuthString.plus_one(value)
  let b = AuthInline.plus_one(a)
  return AuthTable.plus_one(b)
"#;
    let dep_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let main_path = dir.join("main.fuse");
    let util_path = dir.join("util.fuse");
    let dep_string_path = dir.join("deps").join("auth-string").join("lib.fuse");
    let dep_inline_path = dir.join("deps").join("auth-inline").join("lib.fuse");
    let dep_table_path = dir.join("deps").join("auth-table").join("lib.fuse");
    write_project_file(&main_path, main_src);
    write_project_file(&util_path, util_src);
    write_project_file(&dep_string_path, dep_src);
    write_project_file(&dep_inline_path, dep_src);
    write_project_file(&dep_table_path, dep_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let util_uri = path_to_uri(&util_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.open_document(&util_uri, util_src, 1);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        util_diags.is_empty(),
        "dependency manifest syntax variants should resolve dep: imports without diagnostics"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "dependency manifest syntax variants should keep workspace snapshot reuse"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_incrementally_rewires_dependency_import_paths_and_shapes() {
    let dir = temp_project_dir("fuse_lsp_dep_rewire_incremental");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n\n[dependencies]\nAuth = { path = \"./deps/auth\" }\n",
    );

    let main_src = r#"import util from "./util"

fn main():
  print(util.local(1))
"#;
    let util_src = r#"import Auth from "dep:Auth/lib"

fn local(value: Int) -> Int:
  return Auth.plus_one(value)
"#;
    let util_src_v2 = r#"import Ops from "dep:Auth/math/ops"

fn local(value: Int) -> Int:
  return Ops.plus_one(value)
"#;
    let util_src_v3 = r#"import { plus_one } from "dep:Auth/math/ops"

fn local(value: Int) -> Int:
  return plus_one(value)
"#;
    let dep_lib_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;
    let dep_ops_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let main_path = dir.join("main.fuse");
    let util_path = dir.join("util.fuse");
    let dep_lib_path = dir.join("deps").join("auth").join("lib.fuse");
    let dep_ops_path = dir.join("deps").join("auth").join("math").join("ops.fuse");
    write_project_file(&main_path, main_src);
    write_project_file(&util_path, util_src);
    write_project_file(&dep_lib_path, dep_lib_src);
    write_project_file(&dep_ops_path, dep_ops_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let util_uri = path_to_uri(&util_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.open_document(&util_uri, util_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.change_document(&util_uri, util_src_v2, 2);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        util_diags.is_empty(),
        "rewiring dep import path to another dependency module should relink incrementally"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "dep import path rewires should not force full workspace rebuild"
    );

    lsp.change_document(&util_uri, util_src_v3, 3);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        util_diags.is_empty(),
        "changing dep import shape (alias -> named import) should relink incrementally"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "dep import shape changes should not force full workspace rebuild"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_incrementally_materializes_std_modules_during_relink() {
    let dir = temp_project_dir("fuse_lsp_std_incremental");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );

    let main_src = r#"import util from "./util"

fn main():
  print(util.local(1))
"#;
    let util_src = r#"fn local(value: Int) -> Int:
  return value + 1
"#;
    let util_src_v2 = r#"import StdError from "std.Error"

fn local(value: Int) -> Int:
  return value + 1

fn wrap(err: StdError.Error) -> StdError.Error:
  return err
"#;

    let main_path = dir.join("main.fuse");
    let util_path = dir.join("util.fuse");
    write_project_file(&main_path, main_src);
    write_project_file(&util_path, util_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let util_uri = path_to_uri(&util_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.open_document(&util_uri, util_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.change_document(&util_uri, util_src_v2, 2);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        util_diags.is_empty(),
        "new std module imports should relink incrementally"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "std module materialization should not force full workspace rebuild"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_incrementally_loads_new_root_prefix_modules_during_relink() {
    let dir = temp_project_dir("fuse_lsp_root_prefix_incremental");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"src/main.fuse\"\napp = \"Demo\"\n",
    );

    let main_src = r#"import util from "./util"

fn main():
  print(util.local(1))
"#;
    let util_src = r#"fn local(value: Int) -> Int:
  return value + 1
"#;
    let util_src_v2 = r#"import Extra from "root:lib/extra"

fn local(value: Int) -> Int:
  return Extra.plus_one(value)
"#;
    let extra_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let main_path = dir.join("src").join("main.fuse");
    let util_path = dir.join("src").join("util.fuse");
    let extra_path = dir.join("lib").join("extra.fuse");
    write_project_file(&main_path, main_src);
    write_project_file(&util_path, util_src);
    write_project_file(&extra_path, extra_src);

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let util_uri = path_to_uri(&util_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);

    lsp.open_document(&main_uri, main_src, 1);
    assert!(lsp.wait_diagnostics(&main_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.open_document(&util_uri, util_src, 1);
    assert!(lsp.wait_diagnostics(&util_uri).is_empty());
    assert_eq!(workspace_builds(&mut lsp), 1);

    lsp.change_document(&util_uri, util_src_v2, 2);
    let util_diags = lsp.wait_diagnostics(&util_uri);
    assert!(
        util_diags.is_empty(),
        "newly introduced root: imports should relink incrementally"
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        1,
        "new root: import paths should not force full workspace rebuild"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_uses_nearest_manifest_for_nested_docs_workspace() {
    let dir = temp_project_dir("fuse_lsp_nested_manifest");
    fs::create_dir_all(&dir).expect("create temp dir");

    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"app/main.fuse\"\napp = \"Root\"\n",
    );
    write_project_file(&dir.join("app").join("main.fuse"), "fn main():\n  return\n");

    let docs_dir = dir.join("docs");
    write_project_file(
        &docs_dir.join("fuse.toml"),
        "[package]\nentry = \"src/main.fuse\"\napp = \"Docs\"\n",
    );
    write_project_file(
        &docs_dir.join("src").join("main.fuse"),
        "import DocsApi from \"./api\"\n\napp \"Docs\":\n  serve(\"4000\")\n",
    );
    write_project_file(
        &docs_dir.join("src").join("components").join("page.fuse"),
        "fn page_shell(title: String, section: String, slug: String, sidebar: Bool, content: Html) -> Html:\n  return content\n",
    );
    write_project_file(
        &docs_dir.join("src").join("pages").join("home.fuse"),
        "import Page from \"../components/page.fuse\"\n\nfn home_page() -> Html:\n  return Page.page_shell(\"home\", \"home\", \"\", false, html.text(\"ok\"))\n",
    );
    let api_src = "import Home from \"./pages/home.fuse\"\n\nservice DocsApi at \"/\":\n  get \"/\" -> Html:\n    return Home.home_page()\n";
    let api_path = docs_dir.join("src").join("api.fuse");
    write_project_file(&api_path, api_src);

    let root_uri = path_to_uri(&dir);
    let api_uri = path_to_uri(&api_path);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&api_uri, api_src, 1);
    let diags = lsp.wait_diagnostics(&api_uri);
    assert!(
        diags.is_empty(),
        "nested manifest docs file should resolve against docs workspace, got diagnostics: {diags:?}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lsp_reports_module_aware_diagnostics_for_standalone_non_entry_files() {
    let dir = temp_project_dir("fuse_lsp_standalone_docs_file");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_project_file(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"src/main.fuse\"\napp = \"Docs\"\n",
    );
    write_project_file(
        &dir.join("src").join("main.fuse"),
        "app \"Docs\":\n  serve(\"4000\")\n",
    );

    let guide_src = r#"import { NotFound } from "std.Error"

fn load(id: Id) -> String!NotFound:
  return null ?! NotFound(message="missing")
"#;
    let guide_path = dir.join("src").join("guides").join("standalone.fuse");
    let guide_uri = path_to_uri(&guide_path);
    write_project_file(&guide_path, guide_src);

    let root_uri = path_to_uri(&dir);
    let mut lsp = LspClient::spawn_with_root(&root_uri);
    lsp.open_document(&guide_uri, guide_src, 1);
    let diags = lsp.wait_diagnostics(&guide_uri);
    assert!(
        diags.is_empty(),
        "standalone module should be checked with loader semantics, got diagnostics: {diags:?}"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
