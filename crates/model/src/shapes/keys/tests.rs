use super::super::{Model, Uninterpreted};
use super::*;
use crate::testing::{self, Result};

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
