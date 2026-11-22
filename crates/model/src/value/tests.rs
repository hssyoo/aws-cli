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
    assert_eq!(items[0], Value::Float(1.5));
    assert_eq!(items[1], Value::BigInteger("12345678901234567890".into()));
    assert_eq!(items[2], Value::Integer(i64::MIN));
    assert_eq!(items[3], Value::BigInteger("9223372036854775808".into()));
    assert_eq!(items[4], Value::Integer(0));
    assert!(matches!(items[5], Value::Float(zero) if zero == 0.0 && zero.is_sign_negative()));
    assert_eq!(items[6], Value::Float(f64::INFINITY));
    assert_eq!(items[7], Value::Float(100.0));
    assert_eq!(items[8], Value::Float(0.1));
    assert_eq!(items[9], Value::Integer(5));
    Ok(())
}

#[test]
fn objects_keep_insertion_order_and_the_last_duplicate() -> Result<()> {
    let value = Value::from_json(br#"{"b":1,"a":{"y":null,"x":[true,false]},"b":2,"c":"\u00e9\ud83d\ude00"}"#)?;
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
    assert_eq!(inner.get("x"), Some(&Value::List(vec![Value::Bool(true), Value::Bool(false)])));
    assert!(map.get("missing").is_none());
    Ok(())
}

#[test]
fn maps_replace_in_place_and_remove_keeping_order() {
    let mut map = Map::new();
    assert!(map.is_empty());
    assert_eq!(map.insert("a".into(), Value::Integer(1)), None);
    assert_eq!(map.insert("b".into(), Value::Integer(2)), None);
    assert_eq!(map.insert("a".into(), Value::Integer(3)), Some(Value::Integer(1)));
    assert_eq!(map.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(map.len(), 2);
    assert_eq!(map.remove("a"), Some(Value::Integer(3)));
    assert_eq!(map.iter().map(|(k, _)| k).collect::<Vec<_>>(), ["b"]);
    if let Some(value) = map.get_mut("b") {
        *value = Value::Null;
    }
    assert_eq!(map.get("b"), Some(&Value::Null));
}

#[test]
fn json_python_rejects_is_rejected() {
    for text in [&b"{"[..], b"[1,]", b"\"\\x\"", b"", b"1 2"] {
        assert!(Value::from_json(text).is_err(), "{}", String::from_utf8_lossy(text));
    }
}
