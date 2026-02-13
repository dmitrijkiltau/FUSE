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
    Red -> "red"
    Rgb(r, g, b) -> "rgb"

fn describe_user(user: User) -> String:
  match user:
    User(name=n) -> n
"#;
    assert_diags(src, &[]);
}

#[test]
fn checks_task_api_types() {
    let src = r#"
fn main():
  let t = spawn:
    1
  let a = task.id(t)
  let b = task.done(t)
  let c = task.cancel(t)
  let bad = task.id(1)
"#;
    assert_diags(src, &["Error: type mismatch: expected Task<_>, found Int"]);
}

#[test]
fn checks_db_api_types() {
    let src = r#"
fn main():
  let rows = db.query("select 1")
  let first = db.one("select 1")
  db.exec(1)
"#;
    assert_diags(src, &["Error: type mismatch: expected String, found Int"]);
}

#[test]
fn allows_defaulted_trailing_args() {
    let src = r#"
fn greet(name: String, excited: Bool = false):
  if excited:
    print("Hello, ${name}!")
  else:
    print("Hello, ${name}")

fn main():
  greet("world")
"#;
    assert_diags(src, &[]);
}

#[test]
fn allows_box_assignment_through_immutable_binding() {
    let src = r#"
fn main():
  let counter = box 0
  counter = counter + 1
"#;
    assert_diags(src, &[]);
}

#[test]
fn allows_string_concat_with_non_string_operands() {
    let src = r#"
fn main():
  let _ = "rgb " + 1 + "," + 2 + "," + 3
"#;
    assert_diags(src, &[]);
}

#[test]
fn html_block_requires_html_return_type() {
    let src = r#"
fn text(value: String) -> Html:
  return html.text(value)

fn bad(attrs: Map<String, String>, children: List<Html>) -> String:
  return "nope"

fn main():
  bad():
    text("x")
"#;
    assert_diags(
        src,
        &["Error: html block form requires a function that returns Html"],
    );
}

#[test]
fn html_block_children_must_be_html() {
    let src = r#"
fn div(attrs: Map<String, String>, children: List<Html>) -> Html:
  return html.node("div", attrs, children)

fn page() -> Html:
  return div():
    "hello"
"#;
    assert_diags(
        src,
        &["Error: type mismatch: expected List<Html>, found List<String>"],
    );
}

#[test]
fn void_html_tags_reject_children() {
    let src = r#"
fn page() -> Html:
  return meta():
    html.text("x")
"#;
    assert_diags(
        src,
        &[
            "Error: expected at most 1 arguments, got 2",
            "Error: void html tag meta does not accept children",
        ],
    );
}
