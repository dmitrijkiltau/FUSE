use std::path::PathBuf;

use fusec::native::{compile_registry, emit_object_for_app};

fn example_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path
}

#[test]
fn native_object_emits_bytes() {
    let path = example_path("native_heap_literals.fuse");
    let src = std::fs::read_to_string(&path).expect("failed to read example");
    let (registry, diags) = fusec::load_program_with_modules(&path, &src);
    assert!(
        diags.is_empty(),
        "unexpected diagnostics while loading native heap literals: {diags:?}"
    );
    let program = compile_registry(&registry).expect("failed to compile native program");
    let artifact =
        emit_object_for_app(&program, None).expect("failed to emit native object artifact");
    assert!(
        !artifact.object.is_empty(),
        "expected non-empty object bytes"
    );
    assert!(
        !artifact.entry_symbol.is_empty(),
        "expected entry symbol for emitted object"
    );
    assert!(
        !artifact.interned_strings.is_empty(),
        "expected interned strings for emitted object"
    );
}
