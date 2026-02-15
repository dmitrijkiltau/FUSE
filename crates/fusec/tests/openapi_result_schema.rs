use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fuse_rt::json::{JsonValue, decode};

fn write_temp_file(name: &str, ext: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("{name}_{stamp}.{ext}"));
    fs::write(&path, contents).expect("failed to write temp file");
    path
}

fn get_object<'a>(
    value: &'a JsonValue,
    path: &str,
) -> &'a std::collections::BTreeMap<String, JsonValue> {
    let JsonValue::Object(map) = value else {
        panic!("{path}: expected object, got {value:?}");
    };
    map
}

fn get_array<'a>(value: &'a JsonValue, path: &str) -> &'a [JsonValue] {
    let JsonValue::Array(items) = value else {
        panic!("{path}: expected array, got {value:?}");
    };
    items
}

#[test]
fn openapi_request_body_result_uses_tagged_oneof_shape() {
    let program = r#"
type User:
  name: String

service Api at "":
  post "/decode" body Result<User, String> -> String:
    "ok"
"#;
    let path = write_temp_file("fuse_openapi_result_body", "fuse", program);
    let src = fs::read_to_string(&path).expect("failed to read source");
    let (registry, diags) = fusec::load_program_with_modules(&path, &src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let openapi_json =
        fusec::openapi::generate_openapi(&registry).expect("openapi generation failed");
    let doc = decode(&openapi_json).expect("failed to decode openapi json");
    let root = get_object(&doc, "root");
    let paths = get_object(root.get("paths").expect("paths"), "paths");
    let decode_path = get_object(paths.get("/decode").expect("/decode"), "/decode");
    let post = get_object(decode_path.get("post").expect("post"), "post");
    let request_body = get_object(post.get("requestBody").expect("requestBody"), "requestBody");
    let content = get_object(request_body.get("content").expect("content"), "content");
    let app_json = get_object(
        content.get("application/json").expect("application/json"),
        "application/json",
    );
    let schema = get_object(app_json.get("schema").expect("schema"), "schema");
    let one_of = get_array(schema.get("oneOf").expect("oneOf"), "oneOf");
    assert_eq!(one_of.len(), 2, "expected Ok/Err variants");

    let ok_variant = get_object(&one_of[0], "oneOf[0]");
    let err_variant = get_object(&one_of[1], "oneOf[1]");

    let ok_props = get_object(
        ok_variant.get("properties").expect("ok properties"),
        "ok properties",
    );
    let err_props = get_object(
        err_variant.get("properties").expect("err properties"),
        "err properties",
    );

    let ok_type = get_object(ok_props.get("type").expect("ok type"), "ok type");
    let err_type = get_object(err_props.get("type").expect("err type"), "err type");

    let ok_enum = get_array(ok_type.get("enum").expect("ok enum"), "ok enum");
    let err_enum = get_array(err_type.get("enum").expect("err enum"), "err enum");

    assert_eq!(ok_enum, [JsonValue::String("Ok".to_string())]);
    assert_eq!(err_enum, [JsonValue::String("Err".to_string())]);
}
