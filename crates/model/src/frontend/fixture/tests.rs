//! The fixture front-end's honesty checks (DESIGN §4.1, step S2s): each
//! hand-written Smithy twin projects, case by case, as the C2J model the
//! protocol-suite harness builds for that case; a twin that differs in any
//! way botocore reads does not; and `model::schema` reads nothing of
//! `model::frontend`.
use super::{FixtureError, NAME, Overlay, build, load};
use crate::schema::projection::render;
use crate::shapes::Model;
use crate::testing::Result;
use serde_json::{Map, Value as Json, json};
use std::path::{Path, PathBuf};

/// A twin with the suite it copies: `(kind, file, description)`.
struct Twin {
    ast: &'static str,
    overlay: &'static str,
    kind: &'static str,
    file: &'static str,
    description: &'static str,
}

const QUERY: Twin = Twin {
    ast: include_str!("twins/query_maps.json"),
    overlay: include_str!("twins/query_maps.overlay.json"),
    kind: "input",
    file: "query",
    description: "Test cases for QueryMaps operation",
};

const JSON: Twin = Twin {
    ast: include_str!("twins/json_unions.json"),
    overlay: include_str!("twins/json_unions.overlay.json"),
    kind: "input",
    file: "json",
    description: "Test cases for JsonUnions operation",
};

const REST_JSON: Twin = Twin {
    ast: include_str!("twins/rest_json_headers.json"),
    overlay: include_str!("twins/rest_json_headers.overlay.json"),
    kind: "input",
    file: "rest-json",
    description: "Test cases for InputAndOutputWithHeaders operation",
};

const TWINS: [&Twin; 3] = [&QUERY, &JSON, &REST_JSON];

/// The one suite of the twin's file with its description.
fn suite(twin: &Twin) -> Result<Json> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../aws-cli/tests/unit/botocore/protocols")
        .join(twin.kind)
        .join(format!("{}.json", twin.file));
    let suites: Vec<Json> = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut found = suites
        .into_iter()
        .filter(|suite| suite["description"] == twin.description);
    let suite = found.next().ok_or("no suite for the twin")?;
    assert!(found.next().is_none(), "{}: two suites", twin.description);
    Ok(suite)
}

fn object(value: &mut Json) -> Result<&mut Map<String, Json>> {
    Ok(value.as_object_mut().ok_or("not an object")?)
}

/// The protocol-suite harness's model for an input case: the suite without
/// its cases, the case's `given` its only operation.
fn case_document(suite: &Json, case: &Json) -> Result<Json> {
    let name = case["given"]["name"].as_str().unwrap_or("OperationName");
    let mut document = suite.clone();
    let fields = object(&mut document)?;
    fields.remove("cases");
    fields.insert("operations".into(), json!({ name: case["given"] }));
    Ok(document)
}

fn c2j(document: &Json) -> Result<String> {
    Ok(Model::from_value(document)?.projection())
}

fn fixture(ast: &Json, overlay: &Overlay) -> std::result::Result<String, FixtureError> {
    build(ast).map(|schemas| render(&schemas, &overlay.botocore()))
}

fn parse(twin: &Twin) -> Result<(Json, Overlay)> {
    let overlay = Overlay::parse(twin.overlay.as_bytes())?;
    Ok((serde_json::from_str(twin.ast)?, overlay))
}

/// The C2J projection of the twin's suite, from its first case.
fn original(twin: &Twin) -> Result<String> {
    let suite = suite(twin)?;
    let mut document = case_document(&suite, &suite["cases"][0])?;
    object(&mut document)?.remove("description");
    c2j(&document)
}

#[test]
fn every_case_of_each_twin_projects_as_its_c2j_original() -> Result<()> {
    assert_eq!(NAME, "fixture");
    let mut cases = 0;
    for twin in TWINS {
        let overlay = Overlay::parse(twin.overlay.as_bytes())?;
        let schemas = load(twin.ast.as_bytes())?;
        let ours = render(&schemas, &overlay.botocore());
        let suite = suite(twin)?;
        let suite_cases = suite["cases"].as_array().ok_or("no cases")?;
        assert!(!suite_cases.is_empty(), "{}", twin.description);
        for case in suite_cases {
            let id = case["id"].as_str().unwrap_or("?");
            let mut document = case_document(&suite, case)?;
            let harness = c2j(&document)?;
            // The suite's `description` is the harness's own key, not the
            // service's: C2J reads it as uninterpreted, a twin has no such key.
            object(&mut document)?.remove("description");
            let theirs = c2j(&document)?;
            let description = "uninterpreted service \"description\"\n";
            assert_eq!(harness, format!("{theirs}{description}"));
            assert_eq!(ours, theirs, "{} case {id}", twin.description);
            cases += 1;
        }
    }
    assert_eq!(cases, 9 + 9 + 10);
    Ok(())
}

type Edit = dyn FnOnce(&mut Json, &mut Overlay) -> Result<()>;

/// Applies `edit` to a copy of the twin and expects its projection to
/// differ from the C2J original's (or the read to fail when `refused`).
fn mutant(twin: &Twin, refused: bool, edit: Box<Edit>) -> Result<()> {
    let (mut ast, mut overlay) = parse(twin)?;
    let theirs = original(twin)?;
    assert_eq!(fixture(&ast, &overlay)?, theirs, "the unedited twin");
    edit(&mut ast, &mut overlay)?;
    match fixture(&ast, &overlay) {
        Ok(ours) => {
            assert!(!refused, "read a model it must refuse: {ast}");
            assert_ne!(ours, theirs, "an edit the comparison does not see");
        }
        Err(error) => assert!(refused, "refused: {error:?}"),
    }
    Ok(())
}

fn shape<'a>(ast: &'a mut Json, id: &str) -> &'a mut Json {
    &mut ast["shapes"][id]
}

fn members<'a>(ast: &'a mut Json, id: &str) -> Result<&'a mut Map<String, Json>> {
    object(&mut shape(ast, id)["members"])
}

/// Moves a shape's first member to the end.
fn rotate(ast: &mut Json, id: &str, first: &str) -> Result<()> {
    let members = members(ast, id)?;
    let member = members.shift_remove(first).ok_or("no such member")?;
    members.insert(first.into(), member);
    Ok(())
}

const QUERY_INPUT: &str = "aws.protocoltests.query#QueryMapsInput";
const HEADERS_IO: &str = "aws.protocoltests.restjson#InputAndOutputWithHeadersIO";

#[test]
fn a_dropped_member_shows() -> Result<()> {
    mutant(
        &QUERY,
        false,
        Box::new(|ast, _| {
            members(ast, QUERY_INPUT)?.shift_remove("FlattenedMap");
            Ok(())
        }),
    )
}

#[test]
fn a_member_order_swap_shows() -> Result<()> {
    mutant(
        &QUERY,
        false,
        Box::new(|ast, _| rotate(ast, QUERY_INPUT, "MapArg")),
    )
}

#[test]
fn a_dropped_flattened_shows() -> Result<()> {
    mutant(
        &QUERY,
        false,
        Box::new(|ast, _| {
            let traits = &mut members(ast, QUERY_INPUT)?["FlattenedMap"]["traits"];
            object(traits)?.remove("smithy.api#xmlFlattened");
            Ok(())
        }),
    )
}

#[test]
fn a_json_name_on_a_query_model_is_unread() -> Result<()> {
    mutant(
        &QUERY,
        false,
        Box::new(|ast, _| {
            let traits = &mut members(ast, QUERY_INPUT)?["RenamedMapArg"]["traits"];
            *traits = json!({ "smithy.api#jsonName": "Foo" });
            Ok(())
        }),
    )
}

#[test]
fn an_unknown_trait_is_uninterpreted_not_dropped() -> Result<()> {
    let (mut ast, overlay) = parse(&QUERY)?;
    members(&mut ast, QUERY_INPUT)?["MapArg"]["traits"] = json!({ "smithy.api#uniqueItems": {} });
    let ours = fixture(&ast, &overlay)?;
    let line = "uninterpreted shape \"QueryMapsInput\" \"members.MapArg.smithy.api#uniqueItems\"\n";
    assert!(ours.ends_with(line), "{ours}");
    mutant(
        &QUERY,
        false,
        Box::new(|ast, _| {
            shape(ast, "aws.protocoltests.query#AwsQuery")["traits"]["aws.api#arn"] = json!({});
            Ok(())
        }),
    )
}

#[test]
fn a_known_trait_without_a_translation_is_refused() -> Result<()> {
    mutant(
        &QUERY,
        true,
        Box::new(|ast, _| {
            let greeting = shape(ast, "aws.protocoltests.query#GreetingStruct");
            greeting["traits"] = json!({ "smithy.api#retryable": {} });
            Ok(())
        }),
    )?;
    mutant(
        &QUERY,
        true,
        Box::new(|ast, _| {
            let traits = &mut shape(ast, "aws.protocoltests.query#QueryMaps")["traits"];
            traits["aws.protocols#httpChecksum"] = json!({});
            Ok(())
        }),
    )?;
    // Slot traits no twin uses: each refused, not dropped.
    for (id, value) in [
        ("smithy.api#required", json!({})),
        ("smithy.api#httpPayload", json!({})),
        ("smithy.api#timestampFormat", json!("date-time")),
        ("smithy.api#length", json!({ "min": 1 })),
        ("smithy.api#pattern", json!("^a")),
        ("aws.protocols#ec2QueryName", json!("H")),
        ("smithy.rules#contextParam", json!({ "name": "H" })),
    ] {
        mutant(
            &QUERY,
            true,
            Box::new(move |ast, _| {
                let greeting = "aws.protocoltests.query#GreetingStruct";
                members(ast, greeting)?["hi"]["traits"] = json!({ id: value });
                Ok(())
            }),
        )?;
    }
    for id in [
        "smithy.api#auth",
        "smithy.api#endpoint",
        "aws.auth#unsignedPayload",
    ] {
        mutant(
            &QUERY,
            true,
            Box::new(move |ast, _| {
                let traits = &mut shape(ast, "aws.protocoltests.query#QueryMaps")["traits"];
                traits[id] = json!({});
                Ok(())
            }),
        )?;
    }
    // A `c2j#` id is botocore's, never Smithy's.
    mutant(
        &REST_JSON,
        true,
        Box::new(|ast, _| {
            members(ast, HEADERS_IO)?["headerString"]["traits"] =
                json!({ "c2j#location": "header" });
            Ok(())
        }),
    )
}

#[test]
fn an_overlay_change_shows() -> Result<()> {
    mutant(
        &JSON,
        false,
        Box::new(|_, overlay| {
            overlay.target_prefix = Some("Other".into());
            Ok(())
        }),
    )?;
    mutant(
        &QUERY,
        false,
        Box::new(|_, overlay| {
            overlay.signature_version = None;
            Ok(())
        }),
    )
}

#[test]
fn an_enum_order_swap_shows() -> Result<()> {
    let edit =
        |ast: &mut Json, _: &mut Overlay| rotate(ast, "aws.protocoltests.json#FooEnum", "FOO");
    mutant(&JSON, false, Box::new(edit))
}

#[test]
fn a_union_read_as_a_structure_shows() -> Result<()> {
    mutant(
        &JSON,
        false,
        Box::new(|ast, _| {
            shape(ast, "aws.protocoltests.json#MyUnion")["type"] = json!("structure");
            Ok(())
        }),
    )
}

#[test]
fn a_header_name_or_binding_change_shows() -> Result<()> {
    for binding in [
        json!({ "smithy.api#httpHeader": "X-Enums" }),
        json!({ "smithy.api#httpQuery": "X-Enum" }),
        json!({ "smithy.api#httpPrefixHeaders": "X-Enum" }),
        json!({ "smithy.api#httpLabel": {} }),
        json!({ "smithy.api#httpQueryParams": {} }),
        json!({ "smithy.api#httpResponseCode": {} }),
        json!({ "smithy.api#jsonName": "X-Enum" }),
    ] {
        mutant(
            &REST_JSON,
            false,
            Box::new(move |ast, _| {
                members(ast, HEADERS_IO)?["headerEnum"]["traits"] = binding;
                Ok(())
            }),
        )?;
    }
    mutant(
        &REST_JSON,
        false,
        Box::new(|ast, _| {
            let operation = shape(ast, "aws.protocoltests.restjson#InputAndOutputWithHeaders");
            operation["traits"]["smithy.api#http"]["uri"] = json!("/Other");
            Ok(())
        }),
    )
}

#[test]
fn a_model_it_cannot_read_is_refused() -> Result<()> {
    let refused = |ast: Json| assert!(build(&ast).is_err(), "{ast}");
    refused(json!({ "smithy": "1.0", "shapes": {} }));
    refused(json!({ "smithy": "2.0", "shapes": {} }));
    let service = "aws.protocoltests.json#JsonProtocol";
    let (mut ast, _) = parse(&JSON)?;
    shape(&mut ast, service)["operations"] = json!([]);
    refused(ast);
    let (mut ast, _) = parse(&JSON)?;
    shape(&mut ast, "aws.protocoltests.json#Blob")["type"] = json!("bigDecimal");
    refused(ast);
    let (mut ast, _) = parse(&JSON)?;
    shape(&mut ast, service)["traits"]["aws.protocols#restXml"] = json!({});
    refused(ast);
    assert!(Overlay::parse(br#"{"protocl": "json"}"#).is_err());
    Ok(())
}

/// Whether Rust source names the `frontend` module in any spelling.
fn names_frontend(source: &str) -> bool {
    source.contains("frontend")
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

#[test]
fn model_schema_imports_nothing_from_frontend() -> Result<()> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("schema.rs")];
    sources(&src.join("schema"), &mut files)?;
    assert!(files.len() >= 15, "{files:?}");
    let nested = files
        .iter()
        .any(|path| path.ends_with("schema/builder/layout.rs"));
    assert!(nested, "{files:?}");
    for path in &files {
        let source = std::fs::read_to_string(path)?;
        assert!(
            !names_frontend(&source),
            "{} names frontend",
            path.display()
        );
    }
    // Nor through another name: no module re-exports `frontend` under one.
    // This file is skipped: it spells the aliases it looks for.
    let mut all = Vec::new();
    sources(&src, &mut all)?;
    for path in all
        .iter()
        .filter(|path| !path.ends_with("frontend/fixture/tests.rs"))
    {
        let source = std::fs::read_to_string(path)?;
        let aliased = (source.lines())
            .any(|line| line.contains("use ") && names_frontend(line) && line.contains(" as "));
        assert!(!aliased, "{} re-exports frontend", path.display());
    }
    Ok(())
}

#[test]
fn the_import_check_sees_each_spelling() {
    for source in [
        "use crate::frontend::c2j;",
        "use super::super::frontend;",
        "use crate::{frontend::fixture, shapes};",
        "let x = crate :: frontend :: c2j::build(json);",
        "#[path = \"../frontend/c2j.rs\"] mod c2j;",
    ] {
        assert!(names_frontend(source), "{source}");
    }
    assert!(!names_frontend(
        "use crate::shapes::ModelError; // a front-end"
    ));
}
