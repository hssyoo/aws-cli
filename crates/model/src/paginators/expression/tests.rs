use super::*;

/// `text` compiled; text that does not compile reads as a path, which
/// [`paths_parse_as_paths_and_the_subset_compiles`] keeps from hiding.
fn path(text: &str) -> Expression {
    Expression::parse(text).unwrap_or_else(|| Expression {
        text: text.into(),
        compiled: None,
    })
}

/// Identifiers joined by dots are paths; any other expression that
/// compiles within [`PAGINATORS`] is compiled; the rest is refused.
#[test]
fn paths_parse_as_paths_and_the_subset_compiles() {
    for text in ["Things", "Defaults.Marker", "_a.b_2", "A.B.C"] {
        let found = Expression::parse(text).map(|found| (found.to_string(), found.is_path()));
        assert_eq!(found, Some((text.into(), true)), "{text}");
    }
    for text in [
        "Jobs[-1].JobId",
        "NextMarker || Contents[-1].Key",
        "StreamDescription.Shards[-1].ShardId",
        "\"a\"",
        "a | b",
        "a[*].b[].c",
        "a.*",
        "a[?b].c",
        "[a, b]",
        "{a: b}",
        "!a && @",
        "`1`",
    ] {
        let found = Expression::parse(text).map(|found| (found.to_string(), found.is_path()));
        assert_eq!(found, Some((text.into(), false)), "{text}");
    }
    for text in [
        "",
        "a..b",
        ".a",
        "a.",
        "1a",
        "a-b",
        "a[",
        "a[1:]",
        "a[?b > `1`]",
        "length(a)",
        "sort_by(a, &b)",
    ] {
        assert_eq!(Expression::parse(text), None, "{text}");
    }
}

/// An expression beyond a path finds what `jmespath.search` finds: a
/// [`Value`]'s copy with [`Expression::find`]; a borrowed one finds
/// nothing, as [`Lookup`]'s `&Value` documents.
#[test]
fn an_expression_beyond_a_path_finds_what_jmespath_finds() -> Result<(), crate::value::JsonError> {
    let page = Value::from_json(br#"{"Contents": [{"Key": "a"}, {"Key": "b"}], "M": null}"#)?;
    let find = |text: &str| path(text).find(&page);
    assert_eq!(find("Contents[-1].Key"), Value::String("b".into()));
    assert_eq!(find("M || Contents[-1].Key"), Value::String("b".into()));
    assert_eq!(find("Contents[5].Key"), Value::Null);
    assert_eq!(find("Contents[*].Key"), Value::from_json(br#"["a", "b"]"#)?);
    assert_eq!(find("Contents[0]"), Value::from_json(br#"{"Key": "a"}"#)?);
    assert_eq!(find("Contents.Key"), Value::Null);
    assert_eq!(path("Contents[-1].Key").search(&page), None);
    // A path reads as the engine reads it.
    for text in ["Contents", "M", "M.N", "Contents.Key", "Missing.Key"] {
        let engine = jmespath::search(&crate::pyjmespath::Python, text, &page);
        assert_eq!(
            Ok(find(text)),
            engine.map_err(|error| error.class().to_owned()),
            "{text}"
        );
    }
    Ok(())
}

/// `set_value_from_jmespath` refuses an expression with `[`, `]` or `*`
/// before it walks anything.
#[test]
fn set_refuses_what_is_not_a_dotted_lookup() {
    let mut source = Value::Map(Map::new());
    for text in ["Jobs[-1].JobId", "a[*].b", "a.*"] {
        assert_eq!(
            path(text).set(&mut source, Value::Null),
            Err(format!(
                "Invalid expression {text}: Only dotted lookups are supported."
            )),
        );
    }
    assert_eq!(source, Value::Map(Map::new()));
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
