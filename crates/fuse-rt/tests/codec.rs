use std::collections::BTreeMap;

use fuse_rt::codec::{
    EnumType, EnumVariant, StructField, StructType, Type, Value, decode_value, encode_value,
};
use fuse_rt::json::JsonValue;

#[test]
fn decodes_struct_with_paths() {
    let ty = Type::Struct(StructType {
        name: "User".to_string(),
        fields: vec![
            StructField {
                name: "name".to_string(),
                ty: Type::String,
                default: None,
            },
            StructField {
                name: "age".to_string(),
                ty: Type::Int,
                default: None,
            },
        ],
    });

    let mut obj = BTreeMap::new();
    obj.insert("name".to_string(), JsonValue::String("Ada".to_string()));
    let json = JsonValue::Object(obj);

    let err = decode_value(&json, &ty).unwrap_err();
    assert_eq!(err.fields.len(), 1);
    assert_eq!(err.fields[0].path, "age");
    assert_eq!(err.fields[0].code, "missing_field");
}

#[test]
fn decodes_enum_payload() {
    let ty = Type::Enum(EnumType {
        name: "Color".to_string(),
        variants: vec![
            EnumVariant {
                name: "Red".to_string(),
                payload: Vec::new(),
            },
            EnumVariant {
                name: "Rgb".to_string(),
                payload: vec![Type::Int, Type::Int, Type::Int],
            },
        ],
    });

    let json = JsonValue::Object(
        [
            ("type".to_string(), JsonValue::String("Rgb".to_string())),
            (
                "data".to_string(),
                JsonValue::Array(vec![
                    JsonValue::Number(1.0),
                    JsonValue::Number(2.0),
                    JsonValue::Number(3.0),
                ]),
            ),
        ]
        .into_iter()
        .collect(),
    );

    let value = decode_value(&json, &ty).expect("decode enum");
    assert_eq!(
        value,
        Value::Enum {
            name: "Color".to_string(),
            variant: "Rgb".to_string(),
            payload: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
        }
    );
}

#[test]
fn encodes_enum_value() {
    let value = Value::Enum {
        name: "Color".to_string(),
        variant: "Red".to_string(),
        payload: Vec::new(),
    };
    let json = encode_value(&value);
    let obj = match json {
        JsonValue::Object(obj) => obj,
        _ => panic!("expected object"),
    };
    assert_eq!(obj.get("type"), Some(&JsonValue::String("Red".to_string())));
    assert!(!obj.contains_key("data"));
}
