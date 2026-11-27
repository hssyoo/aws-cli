use super::*;
use crate::shapes::{Model, ModelError};
use crate::source::DirSource;
use crate::testing::{self, Checks, Result, Scratch};

const MODEL: &str = r#"{"metadata": {"apiVersion": "2020-01-01", "endpointPrefix": "svc",
  "protocol": "json", "protocols": ["query", "json"], "jsonVersion": "1.1",
  "targetPrefix": "T", "signatureVersion": "v4", "uid": "svc-2020-01-01",
  "serviceId": "Lex Runtime V2", "signingName": "s"},
  "operations": {"Op": {"name": "Op", "http": {"method": "POST", "requestUri": "/"}}},
  "shapes": {}}"#;

/// A search path holding `svc` with `MODEL` and the side files given.
fn service(files: &[(&str, &str)]) -> Result<Scratch> {
    let dir = Scratch::new("overlay")?;
    dir.write("svc/2020-01-01/service-2.json", MODEL.as_bytes())?;
    for (name, text) in files {
        dir.write(&format!("svc/2020-01-01/{name}.json"), text.as_bytes())?;
    }
    Ok(dir)
}

const PAGINATORS: &str = r#"{"pagination": {"Op": {"input_token": "T",
  "output_token": "N", "result_key": "R"}}}"#;
const WAITERS: &str = r#"{"version": 2, "waiters": {"Ready": {"operation": "Op",
  "delay": 1, "maxAttempts": 2, "acceptors": []}}}"#;

/// The overlay holds what botocore reads of `metadata` beside the schema,
/// in the file's order; a model from bytes has no side files.
#[test]
fn a_model_from_bytes_has_its_metadata_and_no_side_files() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let overlay = model.overlay();
    let mut checks = Checks::default();
    let text = |value: &str| Some(Box::<str>::from(value));
    let expected = BotocoreMetadata {
        protocol: text("json"),
        protocols: Some(["query", "json"].map(Box::from).into()),
        json_version: text("1.1"),
        target_prefix: text("T"),
        signature_version: text("v4"),
        uid: text("svc-2020-01-01"),
    };
    checks.eq("metadata", &overlay.metadata, &expected);
    checks.eq("model metadata", model.metadata(), &expected);
    checks.eq("command name", &&*overlay.command_name, &"model");
    checks.eq("paginators", &overlay.paginators, &Paginators::default());
    checks.eq("patches", &overlay.patches, &SchemaPatches::default());
    let source = service(&[("waiters-2", WAITERS)])?;
    let source = DirSource::new(source.path().to_path_buf());
    checks.is("no waiters probed", !overlay.has_waiters(&source));
    let empty = Model::from_json(br#"{"shapes": {}}"#)?;
    checks.eq("absent", empty.metadata(), &BotocoreMetadata::default());
    checks.finish()
}

/// Facts read the schema for the keys a Smithy trait holds.
#[test]
fn facts_read_the_service_schema_beside_the_overlay() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let facts = model.facts();
    let mut checks = Checks::default();
    checks.eq("version", &facts.api_version(), &Some("2020-01-01"));
    checks.eq("prefix", &facts.endpoint_prefix(), &Some("svc"));
    checks.eq("command", &facts.command_name(), &"model");
    checks.eq("metadata", facts.metadata(), model.metadata());
    let hyphenized = facts.hyphenized_service_id();
    checks.eq("words", &hyphenized.as_deref(), &Some("lex-runtime-v2"));
    let absent = Model::from_json(br#"{"shapes": {}}"#)?;
    checks.eq("absent", &absent.facts().hyphenized_service_id(), &None);
    checks.finish()
}

/// A loaded service has its paginators file read with it and its waiters
/// probed once.
#[test]
fn a_loaded_service_reads_its_paginators_and_probes_its_waiters_once() -> Result<()> {
    let dir = service(&[("paginators-1", PAGINATORS), ("waiters-2", WAITERS)])?;
    let source = DirSource::new(dir.path().to_path_buf());
    let model = Model::load(&source, "svc")?;
    let overlay = model.overlay();
    let mut checks = Checks::default();
    checks.eq("command name", &&*overlay.command_name, &"svc");
    let paginators = Paginators::from_json(&serde_json::from_str(PAGINATORS)?);
    checks.eq("paginators", &overlay.paginators, &paginators);
    checks.is("an entry", overlay.paginators.get("Op").is_some());
    checks.is("waiters", overlay.has_waiters(&source));
    std::fs::remove_file(dir.path().join("svc/2020-01-01/waiters-2.json"))?;
    checks.is("probed once", overlay.has_waiters(&source));
    let fresh = Model::load(&source, "svc")?;
    checks.is(
        "a new load probes again",
        !fresh.overlay().has_waiters(&source),
    );
    checks.finish()
}

/// No side files, or a waiters file without a waiter: no paginators and
/// no waiters. A paginators file that cannot be read fails the load.
#[test]
fn absent_side_files_are_empty_and_a_broken_paginators_file_fails() -> Result<()> {
    let mut checks = Checks::default();
    let dir = service(&[])?;
    let source = DirSource::new(dir.path().to_path_buf());
    let model = Model::load(&source, "svc")?;
    checks.eq(
        "paginators",
        &model.overlay().paginators,
        &Paginators::default(),
    );
    checks.is("no waiters file", !model.overlay().has_waiters(&source));
    let dir = service(&[("waiters-2", r#"{"version": 2, "waiters": {}}"#)])?;
    let source = DirSource::new(dir.path().to_path_buf());
    checks.is(
        "empty waiters",
        !Model::load(&source, "svc")?.overlay().has_waiters(&source),
    );
    let dir = service(&[("paginators-1", "{")])?;
    let source = DirSource::new(dir.path().to_path_buf());
    let error = Model::load(&source, "svc").err().map(|e| e.to_string());
    checks.is(
        "broken paginators",
        error.is_some_and(|e| e.contains("paginators-1")),
    );
    checks.finish()
}

#[test]
fn malformed_models_name_what_botocore_would_raise() -> Result<()> {
    let wrap = |shapes: &str| {
        let text = format!(
            concat!(
                r#"{{"metadata":{{}},"operations":{{"Op":{{"input":{{"shape":"In"}}}}}},"#,
                r#""shapes":{{{shapes}}}}}"#
            ),
            shapes = shapes,
        );
        Model::from_json(text.as_bytes())
    };
    type Expect = fn(&ModelError) -> bool;
    let cases: [(&str, Expect); 5] = [
        (
            r#""In":{"type":"structure","members":{"A":{"shape":"Gone"}}}"#,
            |e| matches!(e, ModelError::NoShapeFound(name) if &**name == "Gone"),
        ),
        (
            r#""In":{"members":{}}"#,
            |e| matches!(e, ModelError::InvalidShape(name) if &**name == "In"),
        ),
        (
            r#""In":{"type":"structure","members":{"A":{"location":"header"}}}"#,
            |e| matches!(e, ModelError::InvalidShapeReference(_)),
        ),
        (r#""In":{"type":"string","min":"one"}"#, |e| {
            let ModelError::InvalidTrait { shape, name } = e else {
                return false;
            };
            &**shape == "In" && *name == "min"
        }),
        (
            r#""Other":{"type":"string"}"#,
            |e| matches!(e, ModelError::NoShapeFound(name) if &**name == "In"),
        ),
    ];
    for (shapes, expected) in cases {
        let error = wrap(shapes)
            .err()
            .ok_or_else(|| format!("{shapes} loaded"))?;
        assert!(expected(&error), "{shapes}: {error:?}");
    }
    assert!(matches!(Model::from_json(b"{"), Err(ModelError::Json(_))));
    assert!(matches!(
        Model::from_json(b"[]"),
        Err(ModelError::NotAnObject)
    ));
    Ok(())
}

/// `service`'s overlay holds the paginators file as `Paginators::load`
/// reads it and the metadata as the document holds it; whether it has a
/// paginator.
fn check_overlay(source: &DirSource, service: &str) -> Result<bool> {
    let model = Model::load(source, service).map_err(|e| format!("{service}: {e}"))?;
    let overlay = model.overlay();
    assert_eq!(&*overlay.command_name, service);
    assert_eq!(
        overlay.paginators,
        Paginators::load(source, service)?,
        "{service}"
    );
    let document = load_service_model(source, service, Kind::Service)?;
    let raw = &document.json["metadata"];
    let text = |key: &str| raw.get(key).and_then(Json::as_str);
    let metadata = &overlay.metadata;
    let read = [
        (metadata.protocol.as_deref(), text("protocol")),
        (metadata.json_version.as_deref(), text("jsonVersion")),
        (metadata.target_prefix.as_deref(), text("targetPrefix")),
        (
            metadata.signature_version.as_deref(),
            text("signatureVersion"),
        ),
        (metadata.uid.as_deref(), text("uid")),
    ];
    for (held, written) in read {
        assert_eq!(held, written, "{service}");
    }
    assert!(metadata.protocol.is_some(), "{service}");
    let listed = raw.get("protocols").and_then(Json::as_array);
    let listed: Option<Vec<&str>> =
        listed.map(|names| names.iter().filter_map(Json::as_str).collect());
    let protocols: Option<Vec<&str>> =
        (metadata.protocols.as_ref()).map(|names| names.iter().map(|n| &**n).collect());
    assert_eq!(protocols, listed, "{service}");
    Ok(overlay.paginators.iter().next().is_some())
}

/// Every shipped service loads with its overlay.
#[test]
fn every_service_in_the_repository_loads_with_its_overlay() -> Result<()> {
    let source = DirSource::new(testing::data_dir());
    let services = source.services(Kind::Service)?;
    assert!(services.len() > 400, "{}", services.len());
    let mut paged = 0;
    for service in &services {
        paged += usize::from(check_overlay(&source, service)?);
    }
    assert!(paged > services.len() / 2, "{paged}");
    Ok(())
}
