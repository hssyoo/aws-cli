//! The expected reports in `expected.json` are botocore's own, produced by
//! running `oracle.py` (beside this file) in the blackbox venv:
//!
//! ```text
//! target/blackbox/repo/venv/bin/python crates/model/src/validate/oracle.py \
//!     crates/model/src/validate/fixture.json > crates/model/src/validate/expected.json
//! ```
//!
//! Each case below builds the same parameters as the oracle's entry of the
//! same name.
use super::*;
use crate::shapes::{Model, ShapeId};
use crate::testing::Result;
use crate::value::{Map, Value};
use timestamp::Timestamp;

fn model() -> Result<Model> {
    Ok(Model::from_json(include_bytes!("fixture.json"))?)
}

fn json(text: &str) -> Value {
    Value::from_json(text.as_bytes()).unwrap_or(Value::Null)
}

/// The oracle's `ok` parameters with `member` set to `value`.
fn ok_with(member: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert("Req".into(), Value::String("abc".into()));
    map.insert("Nested".into(), json(r#"{"X":"x"}"#));
    map.insert(member.into(), value);
    Value::Map(map)
}

fn utc_day() -> Value {
    Value::Timestamp(Timestamp::from_unix_seconds(1_577_923_200).unwrap_or(Timestamp::MIN))
}

fn json_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("missing", json("{}")),
        (
            "unknown",
            json(r#"{"Req":"abc","Nested":{"X":"a"},"Bogus":1,"Other":2}"#),
        ),
        (
            "types",
            json(
                r#"{"Req":5,"Nested":[],"Num":"1","Flag":"true","Opt":null,"List":"a",
                "Map":[],"Dbl":"x","Long":1.5}"#,
            ),
        ),
        (
            "ranges",
            json(
                r#"{"Req":"ab","Nested":{"X":"x"},"Num":0,"Long":9,"Dbl":0.25,"List":[],
                "Fmin":1}"#,
            ),
        ),
        ("nested", json(r#"{"Req":"abc","Nested":{"Z":1,"Y":"y"}}"#)),
        ("not_a_dict", json("[]")),
        (
            "everything",
            json(r#"{"Bogus":1,"Num":0,"Req":1,"List":[1],"Union":{}}"#),
        ),
    ]
}

fn member_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("map", ok_with("Map", json(r#"{"k":1,"kk":"x","kkk":0}"#))),
        ("list", ok_with("List", json(r#"["a",5,["x"],"abc"]"#))),
        ("union_empty", ok_with("Union", json("{}"))),
        ("union_two", ok_with("Union", json(r#"{"A":"abc","B":1}"#))),
        ("jsonvalue_ok", ok_with("Json", json(r#"{"a":[1,null]}"#))),
        ("blob", ok_with("Blob", Value::Integer(5))),
        ("blob_ok", ok_with("Blob", Value::String("text".into()))),
        ("timestamps_bad", ok_with("When", json("[1]"))),
        ("timestamp_none", ok_with("When", Value::Null)),
        ("timestamp_nan", ok_with("When", Value::Float(f64::NAN))),
        ("timestamps_ok", ok_with("When", Value::String("12".into()))),
        ("host_label", ok_with("Host", Value::String(String::new()))),
        (
            "repr",
            ok_with("Opt", json(r#"{"it's":["a\nb","q\"'",2.5e-7]}"#)),
        ),
        (
            "big_negative",
            ok_with("Num", json("-99999999999999999999")),
        ),
        ("big_positive", ok_with("Num", json("99999999999999999999"))),
        ("float_for_int", ok_with("Num", Value::Float(1.0))),
    ]
}

fn python_only_cases() -> Vec<(&'static str, Value)> {
    let mut bools = ok_with("Num", Value::Bool(true));
    if let Value::Map(map) = &mut bools {
        map.insert("Dbl".into(), Value::Bool(false));
        map.insert("Long".into(), Value::Bool(true));
    }
    let mut non_ascii = ok_with("Req", Value::String("é😀".into()));
    if let Value::Map(map) = &mut non_ascii {
        map.insert(
            "Opt".into(),
            Value::List(vec![Value::String("é😀\0".into())]),
        );
    }
    let mut document = Map::new();
    let mut inner = Map::new();
    inner.insert("b".into(), Value::Blob(b"x".to_vec()));
    document.insert(
        "a".into(),
        Value::List(vec![Value::Integer(1), Value::Map(inner)]),
    );
    document.insert("c".into(), utc_day());
    document.insert("d".into(), Value::Null);
    document.insert("e".into(), Value::Float(1.5));
    let mut blob_header = Map::new();
    blob_header.insert("a".into(), Value::Blob(b"x".to_vec()));
    vec![
        ("bools_are_ints", bools),
        ("non_ascii", non_ascii),
        ("document", ok_with("Doc", Value::Map(document))),
        ("document_top", ok_with("Doc", Value::Blob(b"raw".to_vec()))),
        ("jsonvalue", ok_with("Json", Value::Map(blob_header))),
        (
            "jsonvalue_datetime",
            ok_with("Json", Value::List(vec![utc_day()])),
        ),
    ]
}

#[test]
fn reports_match_botocore_for_every_error_kind() -> Result<()> {
    let model = model()?;
    let input = model
        .operation("Check")
        .and_then(|op| op.input)
        .ok_or("input")?;
    let expected: serde_json::Value = serde_json::from_slice(include_bytes!("expected.json"))?;
    let expected = expected.as_object().ok_or("expected is not an object")?;
    let mut cases = json_cases();
    cases.extend(member_cases());
    cases.extend(python_only_cases());
    assert_eq!(cases.len(), expected.len());
    for (name, params) in cases {
        let report = match validate(&model, input, &params) {
            Validation::Valid => None,
            Validation::Invalid(report) => Some(report.to_string()),
            Validation::Undecided(reason) => return Err(format!("{name}: {reason:?}").into()),
        };
        let wanted = expected.get(name).ok_or(name)?.as_str();
        assert_eq!(report.as_deref(), wanted, "{name}");
    }
    Ok(())
}

#[test]
fn the_error_message_wraps_the_report_and_no_input_shape_is_valid() -> Result<()> {
    let model = model()?;
    let check = model.operation("Check").ok_or("Check")?;
    let Validation::Invalid(report) = validate_input(&model, check, &json("{}")) else {
        return Err("{} validated".into());
    };
    let text = "Parameter validation failed:\nMissing required parameter in input: \"Req\"\n\
        Missing required parameter in input: \"Nested\"";
    assert_eq!(report.message(), text);
    let bare = model.operation("Bare").ok_or("Bare")?;
    assert!(matches!(
        validate_input(&model, bare, &json("[1]")),
        Validation::Valid
    ));
    Ok(())
}

#[test]
fn timestamps_python_accepts_are_valid() -> Result<()> {
    let model = model()?;
    let input = model
        .operation("Check")
        .and_then(|op| op.input)
        .ok_or("input")?;
    for value in [
        utc_day(),
        Value::String("2020-01-02T03:04:05Z".into()),
        Value::String("2020-01-02".into()),
        Value::String("2020-01-02 03:04:05".into()),
        Value::String("Thu, 02 Jan 2020 03:04:05 GMT".into()),
        Value::String(" 1_000.5 ".into()),
        Value::String("-12e3".into()),
        Value::Float(1.5),
        Value::Integer(-86_400),
        Value::Bool(true),
        Value::Blob(b"12".to_vec()),
    ] {
        let params = ok_with("When", value.clone());
        assert!(
            matches!(validate(&model, input, &params), Validation::Valid),
            "{value:?}"
        );
    }
    Ok(())
}

/// Where botocore's answer depends on dateutil's parser or on an exception
/// it does not catch, the validator declines to decide.
#[test]
fn what_only_python_can_decide_is_undecided() -> Result<()> {
    let model = model()?;
    let input = model
        .operation("Check")
        .and_then(|op| op.input)
        .ok_or("input")?;
    for (member, value) in [
        ("When", Value::String("next tuesday".into())),
        ("When", Value::String("inf".into())),
        // Not float() (CPython strips no U+001C), nor ISO 8601 or RFC 822:
        // dateutil decides.
        ("When", Value::String("\u{1c}5".into())),
        ("When", Value::Float(1e300)),
        ("When", Value::Blob(b"Jan 5".to_vec())),
        ("Big", Value::Integer(1)),
    ] {
        let params = ok_with(member, value.clone());
        let outcome = validate(&model, input, &params);
        assert!(
            matches!(outcome, Validation::Undecided(_)),
            "{value:?}: {outcome:?}"
        );
    }
    Ok(())
}

/// Every arena shape a description reaches, with the named shape it is a
/// child of: the named shapes, their children and the operations'
/// references.
fn reached(model: &Model) -> Vec<(ShapeId, Option<ShapeId>)> {
    use crate::shapes::ShapeKind;
    let mut ids = Vec::new();
    for named in model.named_shapes() {
        ids.push((named, None));
        let children = match &model.shape(named).kind {
            ShapeKind::Structure { .. } => model.members(named).iter().map(|m| m.shape).collect(),
            ShapeKind::List { member } => vec![*member],
            ShapeKind::Map { key, value } => vec![*key, *value],
            _ => Vec::new(),
        };
        ids.extend(children.into_iter().map(|child| (child, Some(named))));
    }
    for operation in model.operations() {
        let references = operation.input.iter().chain(&operation.output);
        ids.extend(references.chain(&operation.errors).map(|id| (*id, None)));
    }
    ids
}

/// What the validator reads of `id`, from the arena.
fn arena_reading(model: &Model, id: ShapeId) -> String {
    use crate::shapes::{Flag as ArenaFlag, ShapeKind};
    let shape = model.shape(id);
    let traits = &shape.traits;
    let kind = match &shape.kind {
        ShapeKind::Structure { .. } if traits.is(ArenaFlag::Document) => "document".to_owned(),
        ShapeKind::Structure { .. } if traits.is(ArenaFlag::Union) => "union".to_owned(),
        ShapeKind::Structure { .. } => "structure".to_owned(),
        ShapeKind::List { .. } => "list".to_owned(),
        ShapeKind::Map { .. } => "map".to_owned(),
        other => format!("{other:?}").to_lowercase(),
    };
    let members: Vec<&str> = model.members(id).iter().map(|m| &*m.name).collect();
    format!(
        "{} {kind} min={:?} required={:?} members={members:?} host={} json={} location={:?}",
        shape.name,
        traits.min.map(|min| min.to_string()),
        traits.required,
        traits.is(ArenaFlag::HostLabel),
        traits.is(ArenaFlag::JsonValue),
        traits.location,
    )
}

/// The same, from the schema the bridge finds.
fn schema_reading(schema: Schema<'_>) -> String {
    let kind = if structure_is(schema, traits::DOCUMENT, ShapeType::Document) {
        "document".to_owned()
    } else if structure_is(schema, traits::UNION, ShapeType::Union) {
        "union".to_owned()
    } else {
        match schema.shape_type() {
            ShapeType::Structure | ShapeType::Document | ShapeType::Union => "structure".to_owned(),
            ShapeType::List => "list".to_owned(),
            ShapeType::Map => "map".to_owned(),
            ShapeType::Unknown => format!("other({:?})", schema.type_name()).to_lowercase(),
            other => format!("{other:?}").to_lowercase(),
        }
    };
    let members: Vec<&str> = schema.members().filter_map(|m| m.member_name()).collect();
    let required: Vec<&str> = schema.required().collect();
    format!(
        "{} {kind} min={:?} required={required:?} members={members:?} host={} json={} \
         location={:?}",
        schema.target().id().name(),
        schema.min().map(|min| min.to_string()),
        schema.flag(Flag::HostLabel),
        schema.flag(Flag::JsonValue),
        schema.location(),
    )
}

/// Where the bridge's reading of `model` differs from the arena's. With
/// `whole`, every id goes through [`root::schema_for`]; else a child goes
/// through its container alone, which keeps the walk linear.
fn bridge_differences(label: &str, model: &Model, whole: bool) -> Vec<String> {
    let mut found = Vec::new();
    for (id, container) in reached(model) {
        let arena = arena_reading(model, id);
        let schema = match container {
            Some(named) if !whole => root::child(model, named, id),
            _ => root::schema_for(model, id),
        };
        let schema = schema.map(schema_reading);
        if schema.as_deref() != Some(arena.as_str()) {
            found.push(format!("{label}: arena {arena}\n  schema {schema:?}"));
        }
    }
    found
}

/// The bridge from an arena id finds the schema the validator reads as the
/// arena did, for every shape of every shipped and fake model.
#[test]
fn every_arena_shape_finds_a_schema_that_reads_the_same() -> Result<()> {
    use crate::source::{DirSource, Kind, Source};
    let mut found = bridge_differences("fixture", &model()?, true);
    let fakes = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
    let mut count = 0;
    for (root, whole) in [(fakes, true), (crate::testing::data_dir(), false)] {
        let source = DirSource::new(root);
        for service in source.services(Kind::Service)? {
            let model = Model::load(&source, &service).map_err(|e| format!("{service}: {e}"))?;
            found.extend(bridge_differences(&service, &model, whole));
            count += 1;
        }
    }
    assert!(count >= 400, "only {count} services");
    assert!(
        found.is_empty(),
        "{} differ:\n{}",
        found.len(),
        found.join("\n")
    );
    Ok(())
}

/// A reference that sets or clears `document` or `union` changes what
/// botocore validates (its `resolve_shape_ref` copies the target and
/// updates it), though a member's type is its target's. Also: the input
/// is validated, not the output; `required` reports in its own order; and
/// `jsonvalue` without a header location is a plain string.
#[test]
fn a_reference_that_changes_document_or_union_validates_as_botocore_reads_it() -> Result<()> {
    let description = serde_json::json!({
        "metadata": {"protocol": "json"},
        "operations": {"Op": {"name": "Op", "input": {"shape": "In"}, "output": {"shape": "Plain"}}},
        "shapes": {
            "In": {"type": "structure", "required": ["Absent2", "Absent1"], "members": {
                "Absent1": {"shape": "Free"},
                "Absent2": {"shape": "Free"},
                "Named": {"shape": "Free", "locationName": "n", "jsonvalue": true},
                "AsDocument": {"shape": "Plain", "document": true},
                "AsUnion": {"shape": "Doc", "document": false, "union": true},
                "AsPlain": {"shape": "Doc", "document": false},
                "Cleared": {"shape": "Choice", "union": false}}},
            "Plain": {"type": "structure", "members": {"A": {"shape": "Free"}}},
            "Doc": {"type": "structure", "document": true,
                    "members": {"A": {"shape": "Free"}, "B": {"shape": "Free"}}},
            "Choice": {"type": "structure", "union": true,
                       "members": {"A": {"shape": "Free"}, "B": {"shape": "Free"}}},
            "Free": {"type": "string"}}});
    let model = Model::from_value(&description)?;
    let operation = model.operation("Op").ok_or("Op")?;
    let mut params = Map::new();
    let mut document = Map::new();
    document.insert("Z".into(), Value::Blob(b"x".to_vec()));
    params.insert("AsDocument".into(), Value::Map(document));
    params.insert("AsUnion".into(), json("{}"));
    params.insert("AsPlain".into(), json(r#"{"Z":1}"#));
    params.insert("Cleared".into(), json(r#"{"A":"a","B":"b"}"#));
    params.insert("Named".into(), Value::Integer(1));
    let Validation::Invalid(report) = validate_input(&model, operation, &Value::Map(params)) else {
        return Err("the parameters validated".into());
    };
    let expected = [
        "Missing required parameter in input: \"Absent2\"",
        "Missing required parameter in input: \"Absent1\"",
        "Invalid type for document parameter Z, value: b'x', type: <class 'bytes'>, valid \
         types: <class 'str'>, <class 'int'>, <class 'bool'>, <class 'float'>, <class 'list'>, \
         <class 'dict'>",
        "Must set one of the following keys for tagged unionstructure AsUnion: A. B.",
        "Unknown parameter in AsPlain: \"Z\", must be one of: A, B",
        "Invalid type for parameter Named, value: 1, type: <class 'int'>, valid types: <class \
         'str'>",
    ];
    assert_eq!(report.errors().collect::<Vec<_>>(), expected);
    Ok(())
}
