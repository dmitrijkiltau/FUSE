use fusec::parse_source;

fn assert_parse_ok(src: &str) {
    let (_program, diags) = parse_source(src);
    if !diags.is_empty() {
        let mut out = String::new();
        for diag in diags {
            out.push_str(&format!(
                "{:?}: {} ({}..{})\n",
                diag.level, diag.message, diag.span.start, diag.span.end
            ));
        }
        panic!("expected parse success, got diagnostics:\n{out}");
    }
}

#[test]
fn parses_indentation_blocks() {
    let src = r#"
fn main(name: String):
  let msg = "user"
  if name == "root":
    let msg = "admin"
    msg
  else:
    msg
  while name == "loop":
    break
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_service_routes() {
    let src = r#"
type User:
  id: Id
  name: String(1..80)

service Users at "/api":
  get "/users/{id: Id}" -> String:
    "ok"

  post "/users" body User -> User:
    body
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_refined_types() {
    let src = r#"
type User:
  name: String(1..80)
  age: Int(0..130) = 18
  nickname: String?

fn load(id: Id) -> User!NotFound:
  User(name="Ada", age=18)
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_interpolated_strings() {
    let src = r#"
fn main(name: String):
  let msg = "hello ${name}"
  let more = "sum ${1 + 2}"
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_spawn_await_box() {
    let src = r#"
fn main():
  let task = spawn:
    let value = box 1
    value
  let out = await task
  out
"#;
    assert_parse_ok(src);
}
