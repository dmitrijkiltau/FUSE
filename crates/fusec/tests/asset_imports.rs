use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fuse_rt::json;
use fusec::loader::ImportedAssetValue;

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}_{nanos}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, text).expect("write file");
}

fn run_fusec(entry: &Path, backend: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fusec"))
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(entry)
        .output()
        .expect("run fusec")
}

#[test]
fn asset_imports_match_between_ast_and_native_backends() {
    let dir = temp_dir("fuse_asset_import_parity");
    let main_path = dir.join("main.fuse");
    write(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    write(&dir.join("README.md"), "# Policy\nline two\n");
    write(
        &dir.join("seed.json"),
        "{\"users\":[{\"id\":1,\"name\":\"Ada\"}],\"enabled\":true}",
    );
    let src = r#"
import Docs from "./README.md"
import Seed from "./seed.json"

config App:
  docs: String = Docs

fn render() -> String:
  return Docs

app "Demo":
  print(App.docs)
  print(render())
  print(json.encode(Seed))
"#;
    write(&main_path, src);

    let ast = run_fusec(&main_path, "ast");
    let native = run_fusec(&main_path, "native");
    assert!(
        ast.status.success(),
        "ast backend failed: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native backend failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        ast.stdout, native.stdout,
        "asset imports should produce identical AST/native output"
    );
    let stdout = String::from_utf8_lossy(&ast.stdout);
    assert!(
        stdout.contains("# Policy"),
        "markdown import should be printed verbatim: {stdout}"
    );
    assert!(
        stdout.contains("{\"enabled\":true,\"users\":[{\"id\":1,\"name\":\"Ada\"}]}"),
        "json import should decode once and re-encode deterministically: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn loader_supports_root_and_dep_asset_imports() {
    let dir = temp_dir("fuse_asset_import_root_dep");
    let fixtures_root = dir.join("deps").join("fixtures");
    let main_path = dir.join("main.fuse");
    write(
        &dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    );
    write(&dir.join("content").join("policy.md"), "root policy\n");
    write(
        &fixtures_root.join("auth").join("login.json"),
        "{\"token\":\"abc123\"}",
    );
    let src = r#"
import Policy from "root:content/policy.md"
import AuthFixture from "dep:Fixtures/auth/login.json"

fn main():
  print(Policy)
  print(json.encode(AuthFixture))
"#;
    let mut deps = HashMap::new();
    deps.insert("Fixtures".to_string(), fixtures_root.clone());

    let (registry, diags) = fusec::load_program_with_modules_and_deps(&main_path, src, &deps);
    assert!(
        diags.is_empty(),
        "unexpected loader diagnostics: {:?}",
        diags.iter().map(|diag| &diag.message).collect::<Vec<_>>()
    );
    let root = registry.root().expect("root module");
    let policy = root.import_assets.get("Policy").expect("Policy asset");
    let auth = root
        .import_assets
        .get("AuthFixture")
        .expect("AuthFixture asset");
    match &policy.value {
        ImportedAssetValue::Markdown(text) => assert_eq!(text, "root policy\n"),
        other => panic!("expected markdown asset, got {other:?}"),
    }
    match &auth.value {
        ImportedAssetValue::Json(value) => {
            assert_eq!(json::encode(value), "{\"token\":\"abc123\"}")
        }
        other => panic!("expected json asset, got {other:?}"),
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn loader_reports_missing_asset_files() {
    let dir = temp_dir("fuse_asset_import_missing");
    let main_path = dir.join("main.fuse");
    let src = r#"
import Docs from "./missing.md"

fn main():
  print(Docs)
"#;
    let (_registry, diags) = fusec::load_program_with_modules(&main_path, src);
    assert!(
        diags
            .iter()
            .any(|diag| diag.message.contains("missing asset file")),
        "expected missing asset diagnostic, got {:?}",
        diags.iter().map(|diag| &diag.message).collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|diag| diag.path.as_ref() == Some(&main_path)),
        "missing asset diagnostic should attach to importer path"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn loader_reports_invalid_json_asset_content() {
    let dir = temp_dir("fuse_asset_import_bad_json");
    let main_path = dir.join("main.fuse");
    let seed_path = dir.join("seed.json");
    write(&seed_path, "{\"users\":[}");
    let src = r#"
import Seed from "./seed.json"

fn main():
  print(json.encode(Seed))
"#;
    let (_registry, diags) = fusec::load_program_with_modules(&main_path, src);
    assert!(
        diags
            .iter()
            .any(|diag| diag.message.contains("invalid json")),
        "expected invalid json diagnostic, got {:?}",
        diags.iter().map(|diag| &diag.message).collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|diag| {
            diag.path
                .as_ref()
                .is_some_and(|path| path.ends_with(seed_path.file_name().expect("seed.json")))
        }),
        "invalid json diagnostic should attach to asset path"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn loader_rejects_unsupported_asset_import_forms_and_extensions() {
    let dir = temp_dir("fuse_asset_import_rejections");
    let main_path = dir.join("main.fuse");
    write(&dir.join("data.json"), "{\"ok\":true}");
    write(&dir.join("notes.txt"), "plain text\n");

    let named_src = r#"
import { data } from "./data.json"

fn main():
  print(0)
"#;
    let (_registry, named_diags) = fusec::load_program_with_modules(&main_path, named_src);
    assert!(
        named_diags.iter().any(|diag| {
            diag.message
                .contains("asset imports only support `import Name from \"path.ext\"`")
        }),
        "expected named asset import rejection, got {:?}",
        named_diags
            .iter()
            .map(|diag| &diag.message)
            .collect::<Vec<_>>()
    );

    let alias_src = r#"
import Data as Alias from "./data.json"

fn main():
  print(0)
"#;
    let (_registry, alias_diags) = fusec::load_program_with_modules(&main_path, alias_src);
    assert!(
        alias_diags.iter().any(|diag| {
            diag.message
                .contains("asset imports only support `import Name from \"path.ext\"`")
        }),
        "expected alias asset import rejection, got {:?}",
        alias_diags
            .iter()
            .map(|diag| &diag.message)
            .collect::<Vec<_>>()
    );

    let ext_src = r#"
import Notes from "./notes.txt"

fn main():
  print(Notes)
"#;
    let (_registry, ext_diags) = fusec::load_program_with_modules(&main_path, ext_src);
    assert!(
        ext_diags
            .iter()
            .any(|diag| diag.message.contains("unsupported import extension .txt")),
        "expected unsupported extension diagnostic, got {:?}",
        ext_diags
            .iter()
            .map(|diag| &diag.message)
            .collect::<Vec<_>>()
    );

    let _ = fs::remove_dir_all(&dir);
}
