use fusec::ast::{Capability, ExprKind, Item, Literal, StmtKind};
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

fn parse_ok(src: &str) -> fusec::ast::Program {
    let (program, diags) = parse_source(src);
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
    program
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

type std.Error.NotFound:
  message: String

fn load(id: Id) -> User!std.Error.NotFound:
  let err = std.Error.NotFound(message="missing")
  User(name="Ada", age=18)
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_refined_regex_and_predicate_constraints() {
    let src = r#"
fn is_slug(value: String) -> Bool:
  return value != ""

type Input:
  slug: String(1..80, regex("^[a-z0-9_-]+$"), predicate(is_slug))
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_without_type_derivations() {
    let src = r#"
type User:
  id: Id
  name: String
  email: String

type PublicUser = User without email, id
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

#[test]
fn parses_index_access() {
    let src = r#"
fn main():
  let items = [1, 2, 3]
  let first = items[0]
  let map = {"a": 1, "b": 2}
  let value = map["a"]
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_range_expr() {
    let src = r#"
fn main():
  let nums = 1..3
  let floats = 1.5..3.5
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_multiline_postfix_chain() {
    let src = r#"
fn get_note(id: Id) -> Map<String, String>!std.Error.NotFound:
  return db
    .from("notes")
    .select(["id", "title", "content"])
    .where("id", "=", id)
    .one()
    ?! std.Error.NotFound(message="not found")
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_multiline_call_args() {
    let src = r#"
fn create_note(id: Id, title: String, content: String):
  db.exec(
    "insert into notes (id, title, content) values (?, ?, ?)",
    [id, title, content],
  )
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_html_block_calls() {
    let src = r#"
fn div(attrs: Map<String, String>, children: List<Html>) -> Html:
  return html.node("div", attrs, children)

fn h1(attrs: Map<String, String>, children: List<Html>) -> Html:
  return html.node("h1", attrs, children)

fn text(value: String) -> Html:
  return html.text(value)

fn page() -> Html:
  let card = div():
    h1():
      text("Hello")
  return card
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_html_attr_shorthand_and_string_children() {
    let src = r#"
fn page(title: String) -> Html:
  let card = div(class="card"):
    "Hello"
    title
  return card
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_inline_if_assignment() {
    let src = r#"
fn main(flag: Bool):
  var class_name = "nav-link"
  if flag: class_name = "nav-link is-active"
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_inline_html_block_child() {
    let src = r#"
fn page() -> Html:
  return span(): "FUSE"
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_multiline_function_signature() {
    let src = r#"
fn page_shell(
  page_title: String,
  current_section: String,
  active_slug: String,
  show_sidebar: Bool,
  content: Html,
) -> Html:
  return content
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_named_call_args_without_commas_on_new_lines() {
    let src = r#"
fn page() -> Html:
  return button(
    class="panel-overlay"
    id="panel-overlay"
    type="button"
    hidden="hidden"
  )
"#;
    assert_parse_ok(src);
}

#[test]
fn parses_module_requires_capabilities() {
    let src = r#"
requires db
requires network, time

fn main():
  0
"#;
    let program = parse_ok(src);
    assert_eq!(program.requires.len(), 3);
    assert_eq!(program.requires[0].capability, Capability::Db);
    assert_eq!(program.requires[1].capability, Capability::Network);
    assert_eq!(program.requires[2].capability, Capability::Time);
}

#[test]
fn lowers_html_block_children_to_canonical_ast_shape() {
    let src = r#"
fn page() -> Html:
  return div(class="hero"):
    "Hello"
"#;
    let program = parse_ok(src);
    let Some(Item::Fn(page)) = program.items.first() else {
        panic!("expected first item to be fn");
    };
    let Some(stmt) = page.body.stmts.first() else {
        panic!("expected function body statement");
    };
    let StmtKind::Return { expr: Some(expr) } = &stmt.kind else {
        panic!("expected return expression");
    };
    let ExprKind::Call { callee, args } = &expr.kind else {
        panic!("expected call expression");
    };
    let ExprKind::Ident(callee_ident) = &callee.kind else {
        panic!("expected identifier callee");
    };
    assert_eq!(callee_ident.name, "div");
    assert_eq!(args.len(), 2, "expected attrs + children arguments");

    let first = &args[0];
    let Some(name) = &first.name else {
        panic!("expected named attr shorthand argument");
    };
    assert_eq!(name.name, "class");
    assert!(!first.is_block_sugar);
    match &first.value.kind {
        ExprKind::Literal(Literal::String(value)) => assert_eq!(value, "hero"),
        other => panic!("expected literal string attr value, got {other:?}"),
    }

    let second = &args[1];
    assert!(second.name.is_none());
    assert!(second.is_block_sugar);
    let ExprKind::ListLit(children) = &second.value.kind else {
        panic!("expected block-sugar children list");
    };
    assert_eq!(children.len(), 1);
    let ExprKind::Call {
        callee: text_callee,
        args: text_args,
    } = &children[0].kind
    else {
        panic!("expected lowered html.text call for string child");
    };
    let ExprKind::Member {
        base: text_base,
        name: text_name,
    } = &text_callee.kind
    else {
        panic!("expected html.text member access");
    };
    let ExprKind::Ident(html_ident) = &text_base.kind else {
        panic!("expected html identifier base");
    };
    assert_eq!(html_ident.name, "html");
    assert_eq!(text_name.name, "text");
    assert_eq!(text_args.len(), 1);
    match &text_args[0].value.kind {
        ExprKind::Literal(Literal::String(value)) => assert_eq!(value, "Hello"),
        other => panic!("expected lowered html.text literal child, got {other:?}"),
    }
}
