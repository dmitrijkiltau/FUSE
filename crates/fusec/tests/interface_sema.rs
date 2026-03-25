use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::{diag::Diag, load_program_with_modules_and_deps, parse_source, sema};

fn analyze_raw(src: &str) -> Vec<Diag> {
    let (program, parse_diags) = parse_source(src);
    if !parse_diags.is_empty() {
        let mut out = String::new();
        for diag in parse_diags {
            out.push_str(&format!(
                "{:?}: {} ({}..{})\n",
                diag.level, diag.message, diag.span.start, diag.span.end
            ));
        }
        panic!("expected parse success, got diagnostics:\n{out}");
    }
    let (_analysis, diags) = sema::analyze_program(&program);
    diags
}

fn analyze_codes(src: &str) -> Vec<String> {
    analyze_raw(src)
        .into_iter()
        .filter_map(|diag| diag.code)
        .collect()
}

fn assert_no_diags(src: &str) {
    let diags = analyze_raw(src);
    assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
}

fn assert_codes_include(src: &str, expected: &[&str]) {
    let actual = analyze_codes(src);
    for code in expected {
        assert!(
            actual.iter().any(|item| item == code),
            "expected diagnostic code {code:?}, got {actual:?}"
        );
    }
}

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

#[test]
fn accepts_instance_and_associated_interface_dispatch() {
    let src = r#"
type ParseError:
  message: String

interface Encodable:
  fn encode() -> String
  fn decode(s: String) -> Self!ParseError

type User:
  name: String

impl Encodable for User:
  fn encode() -> String:
    return self.name
  fn decode(s: String) -> Self!ParseError:
    return User(name=s)

fn round_trip(user: User) -> User!ParseError:
  let encoded = user.encode()
  return User.decode(encoded)
"#;
    assert_no_diags(src);
}

#[test]
fn rejects_duplicate_impl_pairs() {
    let src = r#"
interface Named:
  fn label() -> String

type User:
  name: String

impl Named for User:
  fn label() -> String:
    return self.name

impl Named for User:
  fn label() -> String:
    return self.name
"#;
    assert_codes_include(src, &["FUSE_IMPL_DUPLICATE"]);
}

#[test]
fn rejects_incomplete_impls() {
    let src = r#"
interface Named:
  fn label() -> String
  fn slug() -> String

type User:
  name: String

impl Named for User:
  fn label() -> String:
    return self.name
"#;
    assert_codes_include(src, &["FUSE_IMPL_INCOMPLETE"]);
}

#[test]
fn rejects_signature_mismatches() {
    let src = r#"
interface Factory:
  fn decode(s: String) -> Self

type User:
  name: String

impl Factory for User:
  fn decode(s: Int) -> Self:
    return User(name="Ada")
"#;
    assert_codes_include(src, &["FUSE_IMPL_SIGNATURE_MISMATCH"]);
}

#[test]
fn rejects_interface_names_in_type_positions() {
    let src = r#"
interface Named:
  fn label() -> String

type User:
  name: String

fn bad(user: Named) -> Named:
  let value: Named = user
  return value
"#;
    assert_codes_include(src, &["FUSE_INTERFACE_NOT_A_TYPE"]);
}

#[test]
fn rejects_orphan_impls_across_packages() {
    let root = temp_dir("fuse_interface_orphan");
    let iface_root = root.join("iface");
    let model_root = root.join("model");
    let main_root = root.join("main");

    write(
        &iface_root.join("fuse.toml"),
        "[package]\nname = \"Iface\"\nentry = \"lib.fuse\"\n",
    );
    write(
        &iface_root.join("lib.fuse"),
        r#"interface Encodable:
  fn encode() -> String
"#,
    );

    write(
        &model_root.join("fuse.toml"),
        "[package]\nname = \"Model\"\nentry = \"lib.fuse\"\n",
    );
    write(
        &model_root.join("lib.fuse"),
        r#"type User:
  name: String
"#,
    );

    write(
        &main_root.join("fuse.toml"),
        &format!(
            "[package]\nname = \"Main\"\nentry = \"main.fuse\"\n\n[dependencies]\nIface = \"{}\"\nModel = \"{}\"\n",
            iface_root.display(),
            model_root.display(),
        ),
    );
    let main_src = r#"import { Encodable } from "dep:Iface/lib"
import { User } from "dep:Model/lib"

impl Encodable for User:
  fn encode() -> String:
    return self.name
"#;
    let entry = main_root.join("main.fuse");
    write(&entry, main_src);

    let mut deps = HashMap::new();
    deps.insert("Iface".to_string(), iface_root.clone());
    deps.insert("Model".to_string(), model_root.clone());

    let (registry, load_diags) = load_program_with_modules_and_deps(&entry, main_src, &deps);
    assert!(load_diags.is_empty(), "unexpected load diagnostics: {load_diags:?}");

    let (_analysis, diags) = sema::analyze_registry(&registry);
    let codes: Vec<String> = diags.into_iter().filter_map(|diag| diag.code).collect();
    assert!(
        codes.iter().any(|code| code == "FUSE_IMPL_ORPHAN"),
        "expected FUSE_IMPL_ORPHAN, got {codes:?}"
    );

    let _ = fs::remove_dir_all(root);
}
