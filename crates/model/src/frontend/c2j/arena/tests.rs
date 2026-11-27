//! `c2j_matches_arena`: the C2J front-end's schemas and the arena read
//! every description the same, line for line of the projection, and a
//! description one refuses the other refuses with the same error.
use super::render;
use crate::frontend::c2j;
use crate::shapes::Model;
use crate::source::{DirSource, Kind, Source};
use crate::testing::{self, Result};
use serde_json::{Value as Json, json};
use std::path::Path;

/// The first line the two projections differ on, with its neighbours.
fn first_difference(arena: &str, schema: &str) -> Option<String> {
    let (a, s): (Vec<&str>, Vec<&str>) = (arena.lines().collect(), schema.lines().collect());
    let at = (0..a.len().max(s.len())).find(|i| a.get(*i) != s.get(*i))?;
    Some(format!(
        "line {at}:\n  arena:  {}\n  schema: {}",
        a.get(at).unwrap_or(&"<end>"),
        s.get(at).unwrap_or(&"<end>")
    ))
}

/// The two readings of `json` agree: the same error, or the same
/// projection.
fn agree(label: &str, json: &Json) -> std::result::Result<(), String> {
    let arena = Model::from_value(json);
    let schema = c2j::build(json, &c2j::namespace("t"));
    match (arena, schema) {
        (Ok(model), Ok(_)) => match first_difference(&render(&model), &model.projection()) {
            None => Ok(()),
            Some(difference) => Err(format!("{label}: {difference}")),
        },
        (Err(arena), Err(schema)) if arena.to_string() == schema.to_string() => Ok(()),
        (arena, schema) => Err(format!(
            "{label}: arena {:?}, schema {:?}",
            arena.err().map(|e| e.to_string()),
            schema.err().map(|e| e.to_string())
        )),
    }
}

fn check_all(found: Vec<String>, count: usize, least: usize) -> Result<()> {
    if !found.is_empty() {
        return Err(format!("{} of {count} differ:\n{}", found.len(), found.join("\n")).into());
    }
    assert!(count >= least, "only {count} descriptions read");
    Ok(())
}

/// Every shipped service, `service-2.sdk-extras.json` merged, as
/// `Model::load` reads it.
#[test]
fn c2j_matches_arena_on_every_shipped_model() -> Result<()> {
    let source = DirSource::new(testing::data_dir());
    let services = source.services(Kind::Service)?;
    let mut found = Vec::new();
    for service in &services {
        let model = Model::load(&source, service).map_err(|e| format!("{service}: {e}"))?;
        if let Some(difference) = first_difference(&render(&model), &model.projection()) {
            found.push(format!("{service}: {difference}"));
        }
    }
    check_all(found, services.len(), 400)
}

/// The fake models under `tests/models` (the `z*` protocol fakes).
#[test]
fn c2j_matches_arena_on_the_fake_models() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
    let source = DirSource::new(root);
    let services = source.services(Kind::Service)?;
    let mut found = Vec::new();
    for service in &services {
        let model = Model::load(&source, service).map_err(|e| format!("{service}: {e}"))?;
        if let Some(difference) = first_difference(&render(&model), &model.projection()) {
            found.push(format!("{service}: {difference}"));
        }
    }
    check_all(found, services.len(), 9)
}

/// Each protocol-suite case's description: the suite's, with the case as
/// its only operation, as the suite harness builds it.
fn suite_models() -> Result<Vec<(String, Json)>> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/tests/unit/botocore/protocols");
    let mut models = Vec::new();
    for direction in ["input", "output"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(direction))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<_, _>>()?;
        files.sort();
        for file in files {
            let suite: Json = serde_json::from_slice(&std::fs::read(&file)?)?;
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            for (g, group) in suite.as_array().into_iter().flatten().enumerate() {
                for (c, case) in group["cases"].as_array().into_iter().flatten().enumerate() {
                    let mut model = group.clone();
                    let object = model.as_object_mut().ok_or("group")?;
                    object.remove("cases");
                    let operation = case["given"]["name"].as_str().unwrap_or("OperationName");
                    object.insert(
                        "operations".into(),
                        json!({ operation: case["given"].clone() }),
                    );
                    models.push((format!("{direction}/{name} {g}.{c}"), model));
                }
            }
        }
    }
    Ok(models)
}

#[test]
fn c2j_matches_arena_on_every_protocol_suite_model() -> Result<()> {
    let models = suite_models()?;
    let found: Vec<String> = models
        .iter()
        .filter_map(|(label, model)| agree(label, model).err())
        .collect();
    check_all(found, models.len(), 700)
}

/// Each value of `json` replaced by each wrong-typed stand-in, and each
/// object key removed, one at a time.
fn mutants(json: &Json) -> Vec<Json> {
    fn walk(at: &mut Json, root: &Json, path: &mut Vec<String>, out: &mut Vec<Json>) {
        let stand_ins = [Json::Null, json!("x"), json!({}), json!([1]), json!(1.5)];
        for stand_in in stand_ins {
            let mut mutant = root.clone();
            if let Some(slot) = pointer(&mut mutant, path) {
                *slot = stand_in;
                out.push(mutant);
            }
        }
        if let Some(object) = at.as_object_mut() {
            let keys: Vec<String> = object.keys().cloned().collect();
            for key in keys {
                let mut mutant = root.clone();
                if let Some(Json::Object(parent)) = pointer(&mut mutant, path) {
                    parent.remove(&key);
                    out.push(mutant);
                }
                path.push(key.clone());
                if let Some(child) = object.get_mut(&key) {
                    walk(child, root, path, out);
                }
                path.pop();
            }
        }
    }
    fn pointer<'a>(json: &'a mut Json, path: &[String]) -> Option<&'a mut Json> {
        path.iter().try_fold(json, |at, key| at.get_mut(key))
    }
    let mut out = Vec::new();
    walk(&mut json.clone(), json, &mut Vec::new(), &mut out);
    out
}

/// The first case of every suite group, mutated: wrong-typed values and
/// missing keys at every level give the same error, or read the same.
#[test]
fn c2j_matches_arena_on_mutated_models() -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    let mut count = 0;
    for (label, model) in suite_models()? {
        let group = label.rsplit_once('.').map_or(&*label, |(group, _)| group);
        if !seen.insert(group.to_owned()) {
            continue;
        }
        for mutant in mutants(&model) {
            count += 1;
            if let Err(difference) = agree(&label, &mutant) {
                found.push(format!("{difference}\n  model: {mutant}"));
            }
        }
    }
    found.truncate(5);
    check_all(found, count, 10_000)
}

/// References that change a structure target's `document` or `union`
/// are recorded by both, after the reference's own unknown keys.
#[test]
fn a_reference_that_changes_what_a_structure_is_is_recorded() -> Result<()> {
    let model = json!({"shapes": {
        "S": {"type": "structure", "members": {
            "D": {"shape": "S", "document": true, "newKey": 1},
            "U": {"shape": "Doc", "union": true, "document": false}}},
        "Doc": {"type": "structure", "members": {}, "document": true},
        "Str": {"type": "string", "document": true},
        "L": {"type": "list", "member": {"shape": "Str", "document": false}}}});
    agree("kinds", &model)?;
    let keys: Vec<String> = Model::from_value(&model)?
        .uninterpreted()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        keys,
        [
            "shape S: members.D.newKey",
            "shape S: members.D.document",
            "shape S: members.U.document",
            "shape S: members.U.union",
        ]
    );
    Ok(())
}

/// A falsy key on a reference clears its target's flag, and a `location`
/// on one combines with its target's `locationName`, before either is
/// read: the merge is key by key.
#[test]
fn a_reference_s_keys_overlay_its_target_s_key_by_key() -> Result<()> {
    // The first 17 trait keys are the flags.
    let flags: serde_json::Map<String, Json> = super::super::traits::TRAIT_KEYS[..17]
        .iter()
        .map(|key| ((*key).to_owned(), json!(true)))
        .collect();
    let falsy: serde_json::Map<String, Json> = flags
        .keys()
        .map(|key| (key.clone(), json!(false)))
        .collect();
    let mut cleared = falsy.clone();
    cleared.insert("shape".into(), json!("Flagged"));
    let mut named = flags.clone();
    named.insert("type".into(), json!("structure"));
    named.insert("members".into(), json!({}));
    named.insert("locationName".into(), json!("wire"));
    let model = json!({"shapes": {
        "Flagged": named,
        "S": {"type": "structure", "members": {
            "Cleared": cleared,
            "Header": {"shape": "Flagged", "location": "header"}}}}});
    agree("overlay", &model)?;
    let projection = Model::from_value(&model)?.projection();
    let line = |label: &str| {
        let found = projection.lines().find(|line| line.contains(label));
        found.unwrap_or_default().to_owned()
    };
    assert!(
        !line("member \"Cleared\"").contains("flags="),
        "{projection}"
    );
    assert!(line("member \"Header\"").contains(r#"location="header" locationName="wire""#));
    Ok(())
}

/// The fuzz corpus of `model_json`, through the fuzz body's comparison.
#[test]
fn the_model_json_corpus_reads_the_same() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/model_json");
    let mut count = 0;
    for entry in std::fs::read_dir(root)? {
        crate::fuzz::model_json(&std::fs::read(entry?.path())?);
        count += 1;
    }
    assert!(count > 0);
    Ok(())
}
