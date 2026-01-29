use fusec::{parse_source, sema};

fn analyze(src: &str) -> Vec<String> {
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
    let mut out = Vec::new();
    for diag in diags {
        out.push(format!("{:?}: {}", diag.level, diag.message));
    }
    out.sort();
    out
}

fn assert_diags(src: &str, expected: &[&str]) {
    let actual = analyze(src);
    let mut expected_sorted: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    expected_sorted.sort();
    assert_eq!(actual, expected_sorted);
}

#[test]
fn detects_duplicate_symbols() {
    let src = r#"
type User:
  name: String

fn User():
  0
"#;
    assert_diags(
        src,
        &[
            "Error: duplicate symbol: User",
            "Error: previous definition of User here",
        ],
    );
}

#[test]
fn checks_bang_chain_error_types() {
    let src = r#"
type User:
  name: String

type Err:
  message: String

type Other:
  message: String

fn fetch() -> User?:
  return null

fn load_ok() -> User!Err:
  let user = fetch() ?! Err(message="missing")
  return user

fn load_bad() -> User!Err:
  let user = fetch() ?! Other(message="missing")
  return user
"#;
    assert_diags(src, &["Error: type mismatch: expected Err, found Other"]);
}

#[test]
fn checks_pattern_matching() {
    let src = r#"
enum Color:
  Red
  Rgb(Int, Int, Int)

type User:
  name: String
  age: Int

fn describe(color: Color) -> String:
  match color:
    case Red:
      "red"
    case Rgb(r, g, b):
      "rgb"

fn describe_user(user: User) -> String:
  match user:
    case User(name=n):
      n
"#;
    assert_diags(src, &[]);
}
