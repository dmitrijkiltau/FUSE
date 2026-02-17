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

    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    write_project_file(&util_path, &util_src);
    write_project_file(&main_path, main_src);

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
        2,
        "export-shape changes should fall back to full workspace rebuild"
    );

    let _ = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "util.".len()),
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        2,
        "post-fallback requests should reuse rebuilt workspace snapshot"
    );

    lsp.close_document(&util_uri);
    assert_eq!(
        workspace_builds(&mut lsp),
        2,
        "close should invalidate cache without forcing immediate rebuild"
    );

    let _ = lsp.request(
        "textDocument/completion",
        completion_params(&main_uri, line, col + "util.".len()),
    );
    assert_eq!(
        workspace_builds(&mut lsp),
        3,
        "post-close request should rebuild once back to disk module state"
    );

    lsp.shutdown();
    let _ = fs::remove_dir_all(dir);
}
