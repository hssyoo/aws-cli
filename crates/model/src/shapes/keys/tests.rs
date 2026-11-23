use super::super::{Model, Uninterpreted};
use super::*;
use crate::testing::{self, Result};
use serde_json::{Map, Value as Json};

type Object = Map<String, Json>;

/// A model with an uninterpreted key at every level the arena reads, each
/// beside an ignored key at the same level.
const EVERY_LEVEL: &str = r#"{
  "version": "2.0", "documentation": "d", "newTop": 1,
  "metadata": {"protocol": "json", "ripServiceName": "x", "newMeta": 1},
  "clientContextParams": {"P": {"type": "string", "documentation": "d", "newParam": 1}},
  "operations": {
    "Go": {"name": "Go", "http": {"method": "POST", "requestUri": "/", "newHttp": 1},
      "input": {"shape": "In", "documentation": "d", "newInput": 1},
      "errors": [{"shape": "Oops", "newError": 1}],
      "idempotent": true, "aws.api#longPoll": {},
      "staticContextParams": {"S": {"value": 1, "newStatic": 1}}},
    "Other": {"name": "Other", "http": {"method": "POST", "requestUri": "/"},
      "input": {"shape": "Lone"}}
  },
  "shapes": {
    "In": {"type": "structure", "documentation": "d", "box": true, "newShape": 1,
      "members": {"Items": {"shape": "List", "box": true, "newMember": 1},
                  "Tagged": {"shape": "Leaf", "locationName": "t"}}},
    "List": {"type": "list", "member": {"shape": "Leaf", "newListMember": 1}},
    "Leaf": {"type": "string", "newLeaf": 1},
    "Oops": {"type": "structure", "members": {}, "exception": true,
      "error": {"code": "Oops", "newErrorTrait": 1}},
    "Lone": {"type": "structure", "members": {}}
  }
}"#;

fn keys(found: &[&Uninterpreted]) -> Vec<String> {
    found.iter().map(ToString::to_string).collect()
}

#[test]
fn every_level_records_what_it_does_not_interpret() -> Result<()> {
    let model = Model::from_json(EVERY_LEVEL.as_bytes())?;
    let all: Vec<&Uninterpreted> = model.uninterpreted().iter().collect();
    assert_eq!(
        keys(&all),
        [
            "newTop",
            "metadata.newMeta",
            "clientContextParams.P.newParam",
            "shape In: newShape",
            "shape Leaf: newLeaf",
            "shape Oops: error.newErrorTrait",
            "shape In: members.Items.newMember",
            "shape List: member.newListMember",
            "operation Go: aws.api#longPoll",
            "operation Go: http.newHttp",
            "operation Go: staticContextParams.S.newStatic",
            "operation Go: input.newInput",
            "operation Go: errors.newError",
        ]
    );
    Ok(())
}

/// An operation reaches the service's keys, its own, and those of every
/// shape it reaches, a derived reference's target included; another
/// operation reaches only the service's.
#[test]
fn an_operation_reaches_its_own_its_shapes_and_the_service_s() -> Result<()> {
    let model = Model::from_json(EVERY_LEVEL.as_bytes())?;
    let go = model.operation("Go").ok_or("Go")?;
    let reached = keys(&model.uninterpreted_reached(go));
    assert_eq!(reached.len(), model.uninterpreted().len(), "{reached:?}");
    let other = model.operation("Other").ok_or("Other")?;
    assert_eq!(
        keys(&model.uninterpreted_reached(other)),
        [
            "newTop",
            "metadata.newMeta",
            "clientContextParams.P.newParam"
        ]
    );
    Ok(())
}

/// The arena's fast trait test answers exactly for the trait key lists.
#[test]
fn the_trait_key_match_is_the_trait_key_lists() {
    let listed: Vec<&str> = super::super::traits::trait_keys().collect();
    assert_eq!(listed.len(), 32);
    assert!(listed.iter().all(|key| is_trait_key(key)));
    for key in [SHAPE, SHAPE_IGNORED, REFERENCE_IGNORED].concat() {
        assert!(!is_trait_key(key), "{key}");
    }
}

#[test]
fn a_model_of_known_and_ignored_keys_has_none() -> Result<()> {
    let model = Model::from_json(
        br#"{"version": "2.0", "documentation": "d", "metadata": {"protocol": "json"},
        "operations": {"Go": {"name": "Go", "http": {"method": "POST", "requestUri": "/"},
          "documentation": "d", "readonly": true, "input": {"shape": "In"}}},
        "shapes": {"In": {"type": "structure", "members": {"A": {"shape": "S",
          "documentation": "d", "deprecated": true, "deprecatedMessage": "m"}},
          "sparse": true}, "S": {"type": "string", "pattern": ".*"}}}"#,
    )?;
    assert_eq!(model.uninterpreted(), &[]);
    Ok(())
}

/// Each ignored key is one the vendored botocore reads nowhere on a call's
/// path: no Python source outside `data/` and `docs/` quotes it, except
/// the readers named here. They only document (`documentation`), or read a
/// key of that name somewhere other than where it is ignored: `version` in
/// waiter configs, `deprecated` on an operation (`OperationModel.
/// deprecated`, which the arena reads), a rule-set parameter
/// (`endpoint_provider.py`) and an `endpoints.json` entry (`regions.py`),
/// never on the document.
#[test]
fn the_ignore_lists_name_only_keys_botocore_does_not_act_on() -> Result<()> {
    let botocore = testing::data_dir()
        .parent()
        .ok_or("botocore")?
        .to_path_buf();
    let mut sources = Vec::new();
    let mut directories = vec![botocore.clone()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_dir() && name != "data" && name != "docs" {
                directories.push(path);
            } else if name.ends_with(".py") {
                sources.push((path.clone(), std::fs::read_to_string(&path)?));
            }
        }
    }
    let readers: &[(&str, &[&str])] = &[
        ("documentation", &["model.py", "endpoint_provider.py"]),
        ("version", &["waiter.py"]),
        (
            "deprecated",
            &["model.py", "endpoint_provider.py", "regions.py"],
        ),
    ];
    let ignored = [
        DOCUMENT_IGNORED,
        METADATA_IGNORED,
        OPERATION_IGNORED,
        SHAPE_IGNORED,
        REFERENCE_IGNORED,
        CLIENT_CONTEXT_PARAM_IGNORED,
    ];
    for key in ignored.iter().flat_map(|list| list.iter()) {
        let allowed = readers
            .iter()
            .find(|(name, _)| name == key)
            .map_or(&[][..], |r| r.1);
        for (path, text) in &sources {
            let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let quoted = text.contains(&format!("'{key}'")) || text.contains(&format!("\"{key}\""));
            assert!(
                !quoted || allowed.contains(&file),
                "{key} is read in {}",
                path.display()
            );
        }
    }
    Ok(())
}

static EMPTY: std::sync::LazyLock<Object> = std::sync::LazyLock::new(Object::new);

/// A JSON object, or the empty one.
fn object(value: Option<&Json>) -> &Object {
    value.and_then(Json::as_object).unwrap_or(&EMPTY)
}

/// How often each ignored key occurs in `json`, by list.
fn ignored_occurrences(json: &Json, counts: &mut std::collections::BTreeMap<String, usize>) {
    let mut count = |list: &str, object: &Object, ignored: &[&str]| {
        for key in object.keys().filter(|key| ignored.contains(&key.as_str())) {
            *counts.entry(format!("{list} {key}")).or_default() += 1;
        }
    };
    let Some(document) = json.as_object() else {
        return;
    };
    count("document", document, DOCUMENT_IGNORED);
    count(
        "metadata",
        object(document.get("metadata")),
        METADATA_IGNORED,
    );
    for operation in object(document.get("operations")).values() {
        let operation = object(Some(operation));
        count("operation", operation, OPERATION_IGNORED);
        for key in ["input", "output"] {
            count("reference", object(operation.get(key)), REFERENCE_IGNORED);
        }
    }
    for shape in object(document.get("shapes")).values() {
        let shape = object(Some(shape));
        count("shape", shape, SHAPE_IGNORED);
        for member in object(shape.get("members")).values() {
            count("reference", object(Some(member)), REFERENCE_IGNORED);
        }
    }
    for param in object(document.get("clientContextParams")).values() {
        count(
            "clientContextParam",
            object(Some(param)),
            CLIENT_CONTEXT_PARAM_IGNORED,
        );
    }
}

/// The census of every shipped model (client F1): the operations each
/// uninterpreted key reaches, which a client refuses, and how often each
/// ignored key occurs. It prints the table and asserts its structure: every
/// recorded key is reached by some operation or is the service's, and every
/// ignored key occurs in some shipped model, so the list holds no
/// speculative entry. Run with `RUST_TEST_NOCAPTURE=1` to see the table.
#[test]
fn census_of_the_shipped_models() -> Result<()> {
    use crate::source::{DirSource, Kind, Source, latest_version};
    let source = DirSource::new(testing::data_dir());
    let (mut operations, mut reached) = (0, std::collections::BTreeMap::<String, usize>::new());
    let mut ignored = std::collections::BTreeMap::new();
    let services = source.services(Kind::Service)?;
    for service in &services {
        let version = latest_version(&source, service, Kind::Service)?;
        let bytes = source
            .read(&format!("{service}/{version}/service-2"))?
            .ok_or("model")?;
        let json: Json = serde_json::from_slice(&bytes)?;
        ignored_occurrences(&json, &mut ignored);
        let model = Model::from_value(&json)?;
        operations += model.operations().len();
        for operation in model.operations() {
            for key in model.uninterpreted_reached(operation) {
                *reached.entry(format!("{service} {key}")).or_default() += 1;
            }
        }
        for key in model.uninterpreted() {
            assert!(
                reached.contains_key(&format!("{service} {key}")),
                "{service} {key}"
            );
        }
    }
    let mut table = format!(
        "{} services, {operations} operations\nuninterpreted key -> operations refused\n",
        services.len()
    );
    for (key, count) in &reached {
        table.push_str(&format!("  {key}: {count}\n"));
    }
    table.push_str("ignored key -> occurrences\n");
    for (key, count) in &ignored {
        table.push_str(&format!("  {key}: {count}\n"));
    }
    std::io::Write::write_all(&mut std::io::stderr(), table.as_bytes())?;
    let lists = [
        ("document", DOCUMENT_IGNORED),
        ("metadata", METADATA_IGNORED),
        ("operation", OPERATION_IGNORED),
        ("shape", SHAPE_IGNORED),
        ("reference", REFERENCE_IGNORED),
        ("clientContextParam", CLIENT_CONTEXT_PARAM_IGNORED),
    ];
    for (list, keys) in lists {
        for key in keys {
            assert!(
                ignored.contains_key(&format!("{list} {key}")),
                "{list} {key} never occurs"
            );
        }
    }
    Ok(())
}
