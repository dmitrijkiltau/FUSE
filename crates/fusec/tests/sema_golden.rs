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
fn rejects_implicit_result_error_domain() {
    let src = r#"
type User:
  name: String

fn load() -> User!:
  return User(name="ada")
"#;
    assert_diags(
        src,
        &["Error: result type requires an explicit error domain; use `T!MyError`"],
    );
}

#[test]
fn rejects_non_domain_error_type_in_function_return() {
    let src = r#"
fn load() -> Int!String:
  return 1
"#;
    assert_diags(
        src,
        &[
            "Error: function return type error domains must be declared type/enum names, found String",
        ],
    );
}

#[test]
fn rejects_option_bang_without_explicit_error_value() {
    let src = r#"
type Missing:
  message: String

fn fetch() -> Int?:
  return null

fn load() -> Int!Missing:
  let value = fetch() ?!
  return value
"#;
    assert_diags(
        src,
        &["Error: ?! on Option requires an explicit error value"],
    );
}

#[test]
fn rejects_non_domain_bang_error_value() {
    let src = r#"
type Missing:
  message: String

fn fetch() -> Int?:
  return null

fn load() -> Int!Missing:
  let value = fetch() ?! "missing"
  return value
"#;
    assert_diags(
        src,
        &["Error: ?! error value must be a declared error domain type or enum, found String"],
    );
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
fn reports_removed_task_api() {
    let src = r#"
fn main():
  let t = spawn:
    1
  let a = task.id(t)
  let b = task.done(t)
  let c = task.cancel(t)
"#;
    assert_diags(
        src,
        &[
            "Error: spawned task `t` is not awaited before scope exit",
            "Error: task.cancel was removed in v0.2.0; use spawn + await instead",
            "Error: task.done was removed in v0.2.0; use spawn + await instead",
            "Error: task.id was removed in v0.2.0; use spawn + await instead",
        ],
    );
}

#[test]
fn spawn_rejects_side_effect_builtins() {
    let src = r#"
fn main():
  let t = spawn:
    print("hi")
  await t
"#;
    assert_diags(
        src,
        &["Error: spawn blocks cannot call side-effect builtin print"],
    );
}

#[test]
fn spawn_rejects_input_builtin() {
    let src = r#"
fn main():
  let t = spawn:
    input("name: ")
  await t
"#;
    assert_diags(
        src,
        &["Error: spawn blocks cannot call side-effect builtin input"],
    );
}

#[test]
fn input_builtin_supports_optional_prompt() {
    let src = r#"
fn main():
  let a = input()
  let b = input("name: ")
  print(a + b)
"#;
    assert_diags(src, &[]);
}

#[test]
fn html_input_tag_remains_available_with_named_attrs() {
    let src = r#"
fn field() -> Html:
  return input(type="text")
"#;
    assert_diags(src, &[]);
}

#[test]
fn spawn_rejects_box_capture() {
    let src = r#"
fn main():
  let shared = box 1
  let t = spawn:
    shared
  await t
"#;
    assert_diags(
        src,
        &["Error: spawn blocks cannot capture or use box values"],
    );
}

#[test]
fn spawn_rejects_mutating_captured_outer_state() {
    let src = r#"
fn main():
  var total = 0
  let t = spawn:
    total = 1
  await t
"#;
    assert_diags(
        src,
        &["Error: spawn blocks cannot mutate captured outer state (total)"],
    );
}

#[test]
fn spawn_rejects_detached_expression() {
    let src = r#"
fn main():
  spawn:
    1
"#;
    assert_diags(
        src,
        &["Error: detached task is forbidden; await it or bind it and await before scope exit"],
    );
}

#[test]
fn spawn_rejects_unawaited_binding() {
    let src = r#"
fn main():
  let t = spawn:
    1
"#;
    assert_diags(
        src,
        &["Error: spawned task `t` is not awaited before scope exit"],
    );
}

#[test]
fn spawn_rejects_reassignment_before_await() {
    let src = r#"
fn main():
  var t = spawn:
    1
  t = 2
"#;
    assert_diags(
        src,
        &[
            "Error: spawned task `t` must be awaited before reassignment",
            "Error: spawned task `t` starts here",
            "Error: type mismatch: expected Task<Int>, found Int",
        ],
    );
}

#[test]
fn spawn_binding_can_be_awaited_in_nested_scope() {
    let src = r#"
fn main():
  let t = spawn:
    1
  if true:
    let _ = await t
"#;
    assert_diags(src, &[]);
}

#[test]
fn checks_db_api_types() {
    let src = r#"
requires db

fn main():
  let rows = db.query("select 1")
  let first = db.one("select 1")
  db.exec(1)
"#;
    assert_diags(src, &["Error: type mismatch: expected String, found Int"]);
}

#[test]
fn requires_db_capability_for_db_calls() {
    let src = r#"
fn main():
  db.exec("select 1")
"#;
    assert_diags(
        src,
        &["Error: db call requires capability db; add `requires db` at module top-level"],
    );
}

#[test]
fn requires_network_capability_for_serve_calls() {
    let src = r#"
fn main():
  serve(3000)
"#;
    assert_diags(
        src,
        &[
            "Error: call serve requires capability network; add `requires network` at module top-level",
        ],
    );
}

#[test]
fn rejects_duplicate_requires_declarations() {
    let src = r#"
requires db
requires db

fn main():
  db.exec("select 1")
"#;
    assert_diags(
        src,
        &[
            "Error: duplicate requires declaration for db",
            "Error: previous requires db here",
        ],
    );
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

fn page(name: String) -> Html:
  return div():
    name
"#;
    assert_diags(
        src,
        &["Error: type mismatch: expected List<Html>, found List<String>"],
    );
}

#[test]
fn html_tag_string_literal_children_are_lowered() {
    let src = r#"
fn page() -> Html:
  return div(class="hero"):
    "hello"
"#;
    assert_diags(src, &[]);
}

#[test]
fn html_tag_attr_shorthand_rejects_non_literal_values() {
    let src = r#"
fn page(name: String) -> Html:
  return div(class=name):
    "hello"
"#;
    assert_diags(
        src,
        &["Error: html attribute shorthand only supports string literals"],
    );
}

#[test]
fn html_tag_attr_shorthand_rejects_mixing_positional() {
    let src = r#"
fn page() -> Html:
  return div({"class": "hero"}, id="main")
"#;
    assert_diags(
        src,
        &["Error: cannot mix html attribute shorthand with positional arguments"],
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

#[test]
fn refined_predicate_requires_existing_function() {
    let src = r#"
type Input:
  slug: String(predicate(is_slug))
"#;
    assert_diags(
        src,
        &["Error: unknown predicate function is_slug in current module/import scope"],
    );
}

#[test]
fn refined_predicate_checks_signature() {
    let src = r#"
fn is_slug(value: Int) -> Int:
  return value

type Input:
  slug: String(predicate(is_slug))
"#;
    assert_diags(
        src,
        &[
            "Error: predicate is_slug must return Bool, found Int",
            "Error: predicate is_slug parameter type mismatch: expected String, found Int",
        ],
    );
}

#[test]
fn refined_regex_requires_string_like_base() {
    let src = r#"
type Input:
  age: Int(regex("^[0-9]+$"))
"#;
    assert_diags(
        src,
        &["Error: regex() constraint is only supported for string-like refined bases, found Int"],
    );
}
