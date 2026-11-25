use super::*;

fn path(text: &str) -> Expression {
    Expression::parse(text).unwrap_or_else(|| Expression(text.into()))
}

#[test]
fn identifiers_joined_by_dots_parse_and_nothing_else() {
    for text in ["Things", "Defaults.Marker", "_a.b_2", "A.B.C"] {
        assert_eq!(
            Expression::parse(text).map(|found| found.to_string()),
            Some(text.into())
        );
    }
    for text in [
        "",
        "Jobs[-1].JobId",
        "a..b",
        ".a",
        "a.",
        "1a",
        "a-b",
        "\"a\"",
        "a | b",
    ] {
        assert_eq!(Expression::parse(text), None, "{text}");
    }
}

/// `jmespath.search`: names through dicts; a missing name, a non-dict on
/// the way and a `None` found are all `None`.
#[test]
fn search_reads_through_dicts_and_finds_no_none() -> Result<(), crate::value::JsonError> {
    let page = Value::from_json(br#"{"A": {"B": [1], "N": null}, "S": "text"}"#)?;
    let found = |text: &str| path(text).search(&page).cloned();
    assert_eq!(found("A.B"), Some(Value::List(vec![Value::Integer(1)])));
    assert_eq!(found("A.N"), None);
    assert_eq!(found("A.Missing"), None);
    assert_eq!(found("S.Inner"), None);
    assert_eq!(found("A.B.Inner"), None);
    Ok(())
}

/// `set_value_from_jmespath`: missing dicts are made on the way; the first
/// non-dict raises what Python raises on it.
#[test]
fn set_makes_the_dicts_it_walks_and_raises_on_the_rest() -> Result<(), crate::value::JsonError> {
    let mut source = Value::Map(Map::new());
    assert_eq!(path("A.B").set(&mut source, Value::Integer(1)), Ok(()));
    assert_eq!(path("A.C").set(&mut source, Value::Integer(2)), Ok(()));
    assert_eq!(source, Value::from_json(br#"{"A": {"B": 1, "C": 2}}"#)?);
    let mut page = Value::from_json(br#"{"L": [], "S": "xBx", "T": "x", "I": 1, "Y": "eA="}"#)?;
    if let Value::Map(map) = &mut page {
        map.insert("Y".into(), Value::Blob(vec![1]));
        // A `str` holding a lone surrogate is a `str` like any other.
        let lone = |points: &[u32]| {
            Value::from_text(crate::value::Wtf8Buf::from_code_points(
                points.iter().copied(),
            ))
        };
        map.insert("P".into(), lone(&[0xDC80, 0x42]));
        map.insert("Q".into(), lone(&[0xDC80]));
    }
    for (text, error) in [
        ("L.B", "list indices must be integers or slices, not str"),
        ("S.B.C", "string indices must be integers, not 'str'"),
        ("T.B.C", "'str' object does not support item assignment"),
        ("S.B", "'str' object does not support item assignment"),
        ("P.B.C", "string indices must be integers, not 'str'"),
        ("Q.B.C", "'str' object does not support item assignment"),
        ("Y.B.C", "a bytes-like object is required, not 'str'"),
        ("Y.B", "'bytes' object does not support item assignment"),
        (
            "I.B.C",
            "argument of type 'int' is not a container or iterable",
        ),
        ("I.B", "'int' object does not support item assignment"),
    ] {
        let found = path(text).set(&mut page, Value::Null);
        assert_eq!(found, Err(error.to_owned()), "{text}");
    }
    Ok(())
}
