use fusec::interp::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fusec::native::value::{NativeHeap, NativeTag, NativeValue};

#[test]
fn native_value_roundtrip_numeric() {
    let mut heap = NativeHeap::new();
    let values = [
        Value::Int(-42),
        Value::Bool(true),
        Value::Float(3.5),
        Value::Null,
    ];
    for value in values {
        let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
        let round = native.to_value(&heap).expect("decode failed");
        assert_value_eq(&value, &round);
    }
}

#[test]
fn native_value_roundtrip_string() {
    let mut heap = NativeHeap::new();
    let value = Value::String("hello".to_string());
    let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
    assert_eq!(native.tag, NativeTag::Heap);
    let round = native.to_value(&heap).expect("decode failed");
    assert_value_eq(&value, &round);
}

#[test]
fn native_value_roundtrip_list() {
    let mut heap = NativeHeap::new();
    let value = Value::List(vec![
        Value::Int(1),
        Value::Bool(false),
        Value::Float(2.5),
        Value::String("ok".to_string()),
        Value::Null,
    ]);
    let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
    let round = native.to_value(&heap).expect("decode failed");
    assert_value_eq(&value, &round);
}

#[test]
fn native_value_roundtrip_map() {
    let mut heap = NativeHeap::new();
    let mut map = HashMap::new();
    map.insert("a".to_string(), Value::Int(10));
    map.insert("b".to_string(), Value::Float(3.25));
    map.insert("c".to_string(), Value::String("ok".to_string()));
    let value = Value::Map(map);
    let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
    let round = native.to_value(&heap).expect("decode failed");
    assert_value_eq(&value, &round);
}

#[test]
fn native_value_roundtrip_struct() {
    let mut heap = NativeHeap::new();
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Ada".to_string()));
    fields.insert("age".to_string(), Value::Int(42));
    let value = Value::Struct {
        name: "User".to_string(),
        fields,
    };
    let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
    let round = native.to_value(&heap).expect("decode failed");
    assert_value_eq(&value, &round);
}

#[test]
fn native_value_roundtrip_enum() {
    let mut heap = NativeHeap::new();
    let value = Value::Enum {
        name: "Color".to_string(),
        variant: "Rgb".to_string(),
        payload: vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::String("ok".to_string()),
        ],
    };
    let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
    let round = native.to_value(&heap).expect("decode failed");
    assert_value_eq(&value, &round);
}

#[test]
fn native_value_roundtrip_boxed() {
    let mut heap = NativeHeap::new();
    let value = Value::Boxed(Arc::new(Mutex::new(Value::Int(7))));
    let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
    let round = native.to_value(&heap).expect("decode failed");
    assert_value_eq(&value, &round);
}

#[test]
fn native_value_roundtrip_result() {
    let mut heap = NativeHeap::new();
    let ok = Value::ResultOk(Box::new(Value::String("ok".to_string())));
    let native_ok = NativeValue::from_value(&ok, &mut heap).expect("encode failed");
    let round_ok = native_ok.to_value(&heap).expect("decode failed");
    assert_value_eq(&ok, &round_ok);

    let err = Value::ResultErr(Box::new(Value::Int(5)));
    let native_err = NativeValue::from_value(&err, &mut heap).expect("encode failed");
    let round_err = native_err.to_value(&heap).expect("decode failed");
    assert_value_eq(&err, &round_err);
}

fn assert_value_eq(expected: &Value, actual: &Value) {
    match (expected, actual) {
        (Value::Int(a), Value::Int(b)) => assert_eq!(a, b),
        (Value::Bool(a), Value::Bool(b)) => assert_eq!(a, b),
        (Value::Float(a), Value::Float(b)) => assert!((a - b).abs() < f64::EPSILON),
        (Value::String(a), Value::String(b)) => assert_eq!(a, b),
        (Value::Null, Value::Null) => {}
        (Value::List(a), Value::List(b)) => {
            assert_eq!(a.len(), b.len());
            for (left, right) in a.iter().zip(b.iter()) {
                assert_value_eq(left, right);
            }
        }
        (Value::Map(a), Value::Map(b)) => {
            assert_eq!(a.len(), b.len());
            for (key, value) in a {
                let other = b.get(key).expect("missing map key");
                assert_value_eq(value, other);
            }
        }
        (
            Value::Struct {
                name: a_name,
                fields: a,
            },
            Value::Struct {
                name: b_name,
                fields: b,
            },
        ) => {
            assert_eq!(a_name, b_name);
            assert_eq!(a.len(), b.len());
            for (key, value) in a {
                let other = b.get(key).expect("missing struct field");
                assert_value_eq(value, other);
            }
        }
        (
            Value::Enum {
                name: a_name,
                variant: a_variant,
                payload: a_payload,
            },
            Value::Enum {
                name: b_name,
                variant: b_variant,
                payload: b_payload,
            },
        ) => {
            assert_eq!(a_name, b_name);
            assert_eq!(a_variant, b_variant);
            assert_eq!(a_payload.len(), b_payload.len());
            for (left, right) in a_payload.iter().zip(b_payload.iter()) {
                assert_value_eq(left, right);
            }
        }
        (Value::Boxed(a), Value::Boxed(b)) => {
            let a_guard = a.lock().expect("box lock");
            let b_guard = b.lock().expect("box lock");
            assert_value_eq(&a_guard, &b_guard);
        }
        (Value::ResultOk(a), Value::ResultOk(b)) => {
            assert_value_eq(a.as_ref(), b.as_ref());
        }
        (Value::ResultErr(a), Value::ResultErr(b)) => {
            assert_value_eq(a.as_ref(), b.as_ref());
        }
        other => panic!("unexpected value pair: {other:?}"),
    }
}
