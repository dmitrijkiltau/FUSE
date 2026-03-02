use fusec::ir::lower::lower_program;
use fusec::loader::ModuleMap;
use fusec::parse_source;

fn lower_without_modules(src: &str) -> Result<(), Vec<String>> {
    let (program, parse_diags) = parse_source(src);
    assert!(
        parse_diags.is_empty(),
        "unexpected parse diagnostics: {parse_diags:?}"
    );
    lower_program(&program, &ModuleMap::default()).map(|_| ())
}

#[test]
fn lowers_index_call_target_without_placeholder_error() {
    let src = r#"
app "demo":
  [1][0]()
"#;
    let result = lower_without_modules(src);
    assert!(result.is_ok(), "lowering failed: {result:?}");
}

#[test]
fn lowers_optional_index_call_target_without_placeholder_error() {
    let src = r#"
app "demo":
  [1]?[0]()
"#;
    let result = lower_without_modules(src);
    assert!(result.is_ok(), "lowering failed: {result:?}");
}

#[test]
fn lowers_optional_member_call_target_without_placeholder_error() {
    let src = r#"
app "demo":
  {"f": 1}?.f()
"#;
    let result = lower_without_modules(src);
    assert!(result.is_ok(), "lowering failed: {result:?}");
}
