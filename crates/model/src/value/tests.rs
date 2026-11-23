use super::*;
use crate::testing::Result;

#[test]
fn numbers_follow_python_json_loads() -> Result<()> {
    let value = Value::from_json(
        br#"[1.50, 12345678901234567890, -9223372036854775808, 9223372036854775808, -0, -0.0,
            1e400, 1E2, 0.1, 5]"#,
    )?;
    let Value::List(items) = value else {
        return Err("not a list".into());
    };
    let negative_zero =
        matches!(items[5], Value::Float(zero) if zero == 0.0 && zero.is_sign_negative());
    assert!(negative_zero, "{:?}", items[5]);
    let expected = [
        Value::Float(1.5),
        Value::BigInteger("12345678901234567890".into()),
        Value::Integer(i64::MIN),
        Value::BigInteger("9223372036854775808".into()),
        Value::Integer(0),
        Value::Float(-0.0),
        Value::Float(f64::INFINITY),
        Value::Float(100.0),
        Value::Float(0.1),
        Value::Integer(5),
    ];
    assert_eq!(items, expected);
    Ok(())
}

#[test]
fn objects_keep_insertion_order_and_the_last_duplicate() -> Result<()> {
    let value = Value::from_json(
        br#"{"b":1,"a":{"y":null,"x":[true,false]},"b":2,"c":"\u00e9\ud83d\ude00"}"#,
    )?;
    let Value::Map(map) = value else {
        return Err("not a map".into());
    };
    let keys: Vec<&str> = map.keys().collect();
    assert_eq!(keys, ["b", "a", "c"]);
    assert_eq!(map.get("b"), Some(&Value::Integer(2)));
    assert_eq!(map.get("c"), Some(&Value::String("é😀".into())));
    let Some(Value::Map(inner)) = map.get("a") else {
        return Err("a is not a map".into());
    };
    assert_eq!(inner.get("y"), Some(&Value::Null));
    assert_eq!(
        inner.get("x"),
        Some(&Value::List(vec![Value::Bool(true), Value::Bool(false)]))
    );
    assert!(map.get("missing").is_none());
    Ok(())
}

#[test]
fn maps_replace_in_place_and_remove_keeping_order() {
    let mut map = Map::new();
    let empty = map.is_empty();
    let inserted = [
        map.insert("a".into(), Value::Integer(1)),
        map.insert("b".into(), Value::Integer(2)),
        map.insert("a".into(), Value::Integer(3)),
    ];
    assert!(empty);
    assert_eq!(inserted, [None, None, Some(Value::Integer(1))]);
    assert_eq!(
        (map.keys().collect::<Vec<_>>(), map.len()),
        (vec!["a", "b"], 2)
    );
    assert_eq!(map.remove("a"), Some(Value::Integer(3)));
    if let Some(value) = map.get_mut("b") {
        *value = Value::Null;
    }
    let entries: Vec<(&str, &Value)> = map.iter().collect();
    assert_eq!(entries, [("b", &Value::Null)]);
}

#[test]
fn json_python_rejects_is_rejected() {
    for text in [&b"{"[..], b"[1,]", b"\"\\x\"", b"", b"1 2"] {
        assert!(
            Value::from_json(text).is_err(),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
}

/// The accessors read one kind each, as the readers of endpoint
/// properties, error bodies and rule sets need them.
#[test]
fn accessors_read_their_kind_and_nothing_else() -> Result<()> {
    let value = Value::from_json(br#"{"a": {"b": "text"}, "l": ["x"], "n": 1}"#)?;
    assert_eq!(
        value
            .get("a")
            .and_then(|a| a.get("b"))
            .and_then(Value::as_str),
        Some("text")
    );
    let list = value.get("l").and_then(Value::as_list).ok_or("list")?;
    assert_eq!(list.first().and_then(Value::as_str), Some("x"));
    assert!(value.as_map().is_some_and(|map| map.len() == 3));
    let number = value.get("n").ok_or("n")?;
    assert_eq!(
        (number.as_str(), number.as_map(), number.as_list()),
        (None, None, None)
    );
    assert!(number.get("x").is_none() && value.get("missing").is_none());
    Ok(())
}
