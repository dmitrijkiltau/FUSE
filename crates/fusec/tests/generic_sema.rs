use fusec::{parse_source, sema};

fn analyze_raw(src: &str) -> Vec<fusec::diag::Diag> {
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

// ---------------------------------------------------------------------------
// Happy-path: generic fn parses and type-checks correctly
// ---------------------------------------------------------------------------

#[test]
fn generic_fn_type_checks() {
    let src = r#"
interface Encodable:
  fn encode() -> String
  fn from_text(s: String) -> Self

type User:
  name: String

impl Encodable for User:
  fn encode() -> String:
    return self.name
  fn from_text(s: String) -> Self:
    return User(name=s)

fn round_trip<T>(text: String) -> String where T: Encodable:
  let decoded = T.from_text(text)
  return decoded.encode()
"#;
    assert_no_diags(src);
}

// ---------------------------------------------------------------------------
// FUSE_GENERIC_CALL_TYPE_ARG: wrong number of type args at call site
// ---------------------------------------------------------------------------

#[test]
fn rejects_wrong_type_arg_count() {
    let src = r#"
fn identity<T>(x: Int) -> Int:
  return x

fn main() -> Int:
  return identity<Int, Int>(1)
"#;
    assert_codes_include(src, &["FUSE_GENERIC_CALL_TYPE_ARG"]);
}

#[test]
fn rejects_type_args_on_non_generic_fn() {
    let src = r#"
fn plain(x: Int) -> Int:
  return x

fn main() -> Int:
  return plain<Int>(1)
"#;
    assert_codes_include(src, &["FUSE_GENERIC_CALL_TYPE_ARG"]);
}

// ---------------------------------------------------------------------------
// FUSE_WHERE_UNKNOWN_INTERFACE: bad interface name in where clause
// ---------------------------------------------------------------------------

#[test]
fn rejects_unknown_interface_in_where_clause() {
    let src = r#"
fn process<T>(x: String) -> String where T: DoesNotExist:
  return x
"#;
    assert_codes_include(src, &["FUSE_WHERE_UNKNOWN_INTERFACE"]);
}

// ---------------------------------------------------------------------------
// FUSE_WHERE_MULTI_CONSTRAINT: multiple constraints on one type param
// ---------------------------------------------------------------------------

#[test]
fn rejects_multi_constraint_on_same_type_param() {
    let src = r#"
interface Encodable:
  fn encode() -> String

interface Printable:
  fn print() -> String

fn process<T>(x: String) -> String where T: Encodable, T: Printable:
  return x
"#;
    assert_codes_include(src, &["FUSE_WHERE_MULTI_CONSTRAINT"]);
}

// ---------------------------------------------------------------------------
// FUSE_GENERIC_DUPLICATE_TYPE_PARAM: duplicate type param names
// ---------------------------------------------------------------------------

#[test]
fn rejects_duplicate_type_params() {
    let src = r#"
fn process<T, T>(x: String) -> String:
  return x
"#;
    assert_codes_include(src, &["FUSE_GENERIC_DUPLICATE_TYPE_PARAM"]);
}

// ---------------------------------------------------------------------------
// FUSE_GENERIC_INFERENCE: type cannot be inferred (no type args, no usage)
// ---------------------------------------------------------------------------

#[test]
fn rejects_unresolved_type_inference() {
    let src = r#"
fn identity<T>(x: Int) -> Int:
  return x

fn main() -> Int:
  return identity(1)
"#;
    assert_codes_include(src, &["FUSE_GENERIC_INFERENCE"]);
}
