use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fusec::ast::{ExprKind, Item, Literal, StmtKind};

fn write_temp_file(name: &str, ext: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    path.push(format!("{name}_{stamp}.{ext}"));
    fs::write(&path, contents).expect("failed to write temp file");
    path
}

#[test]
fn html_attr_shorthand_is_canonicalized_in_loaded_registry() {
    let src = r#"
app "demo":
  let view = button(aria_label="Close navigation"):
    "Open"
  print(html.render(view))
"#;
    let path = write_temp_file("fuse_canonicalize", "fuse", src);
    let (registry, diags) = fusec::load_program_with_modules(&path, src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let root = registry.root().expect("root module");
    let app = root
        .program
        .items
        .iter()
        .find_map(|item| match item {
            Item::App(app) => Some(app),
            _ => None,
        })
        .expect("app item");
    let first_stmt = app.body.stmts.first().expect("let statement");
    let value_expr = match &first_stmt.kind {
        StmtKind::Let { expr, .. } => expr,
        other => panic!("expected let statement, got {other:?}"),
    };
    let ExprKind::Call { args, .. } = &value_expr.kind else {
        panic!("expected call expression");
    };
    assert_eq!(args.len(), 2, "expected attrs + children");
    assert!(
        args.iter().all(|arg| arg.name.is_none()),
        "canonical args must be positional"
    );

    let ExprKind::MapLit(entries) = &args[0].value.kind else {
        panic!("expected canonical attrs map");
    };
    assert_eq!(entries.len(), 1, "expected one canonical attr");
    let (key, value) = &entries[0];
    let ExprKind::Literal(Literal::String(key)) = &key.kind else {
        panic!("expected string attr key");
    };
    let ExprKind::Literal(Literal::String(value)) = &value.kind else {
        panic!("expected string attr value");
    };
    assert_eq!(key, "aria-label");
    assert_eq!(value, "Close navigation");

    let ExprKind::ListLit(children) = &args[1].value.kind else {
        panic!("expected canonical children list");
    };
    assert_eq!(children.len(), 1, "expected one lowered child");
}
