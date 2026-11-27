#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::ServiceSource;
use crate::testing::{Result, parsed, replacing_all, with_models};

fn outcome(
    source: &ServiceSource<'_>,
    customizations: &[&'static Customization],
    argv: &[&str],
) -> std::result::Result<Outcome, PythonReason> {
    admit(source.models(), customizations, &parsed(source, argv))
}

/// The admitted call of `argv`, which no handler raises on.
fn admitted(
    source: &ServiceSource<'_>,
    customizations: &[&'static Customization],
    argv: &[&str],
) -> std::result::Result<Admitted, PythonReason> {
    match outcome(source, customizations, argv)? {
        Outcome::Call(admitted) => Ok(*admitted),
        Outcome::Raised { failure, .. } => panic!("raised: {failure:?}"),
    }
}

/// A covered operation of an implemented protocol is admitted with its
/// request and the client's validation of it.
#[test]
fn a_covered_operation_is_admitted_with_its_request() -> Result<()> {
    with_models(|models| {
        let everything = replacing_all(&models, "zquery", "get-thing");
        let source = crate::testing::source(models, everything);
        let found = admitted(
            &source,
            everything,
            &["zquery", "get-thing", "--thing-id", "a"],
        )
        .unwrap_or_else(|reason| panic!("{reason:?}"));
        assert_eq!((&*found.service, &*found.operation), ("zquery", "GetThing"));
        assert_eq!(found.validation, Validation::Valid);
    })
}

/// Each stage refuses with its own reason: an uncovered customization, a
/// paginator whose expression the native paginator does not run (zquery's
/// ListJobs reads its token as `Jobs[-1].JobId`), an unimplemented protocol.
#[test]
fn each_stage_names_its_refusal() -> Result<()> {
    with_models(|models| {
        let everything = replacing_all(&models, "zquery", "list-jobs");
        let source = crate::testing::source(models, everything);
        let refused = |customizations: &[&'static Customization], argv: &[&str]| {
            admitted(&source, customizations, argv).err()
        };
        assert_eq!(
            refused(&[], &["zquery", "get-thing", "--thing-id", "a"]),
            Some(PythonReason::UnportedCustomization)
        );
        assert_eq!(
            refused(everything, &["zquery", "list-jobs"]),
            Some(PythonReason::Paginator)
        );
        assert_eq!(
            refused(&[], &["s3api", "list-buckets"]),
            Some(PythonReason::UnsupportedProtocol)
        );
    })
}

/// A paginated operation is admitted with its paginator when paging is on,
/// and as one call under `--no-paginate`; a page option beside
/// `--no-paginate` is what its handler raised, with no call.
#[test]
fn a_paginated_operation_is_admitted_with_its_paginator() -> Result<()> {
    with_models(|models| {
        let everything = replacing_all(&models, "zjson", "list-things");
        let source = crate::testing::source(models, everything);
        let found = admitted(
            &source,
            everything,
            &["zjson", "list-things", "--max-items", "2"],
        )
        .unwrap_or_else(|reason| panic!("{reason:?}"));
        let paginated = found.paginated.unwrap_or_else(|| panic!("no paginator"));
        assert_eq!(paginated.config.result_key, ["Things"]);
        assert_eq!(paginated.config.limit_key.as_deref(), Some("MaxResults"));
        assert_eq!(paginated.pagination.max_items, Some(Value::Integer(2)));
        assert!(!found.warned);
        let single = admitted(
            &source,
            everything,
            &["zjson", "list-things", "--no-paginate"],
        )
        .unwrap_or_else(|reason| panic!("{reason:?}"));
        assert_eq!(single.paginated, None);
        let raised = outcome(
            &source,
            everything,
            &["zjson", "list-things", "--no-paginate", "--max-items", "2"],
        );
        assert!(
            matches!(&raised, Ok(Outcome::Raised { command, .. }) if command == "zjson"),
            "{raised:?}"
        );
    })
}

/// The paginator entries the native paginator runs: plain member names
/// (zjson's ListThings) and dotted paths (elasticache's
/// DescribeEngineDefaultParameters); real JMESPath (kinesis's
/// DescribeStream) is refused, and an operation without one has none.
#[test]
fn paginators_of_names_and_dotted_paths_are_run() -> Result<()> {
    with_models(|models| {
        let entry = |command: &str, operation: &str| {
            let service = models
                .service(command)
                .unwrap_or_else(|| panic!("{command}"));
            let found = service
                .operation(operation)
                .unwrap_or_else(|| panic!("{operation}"));
            service
                .paginator(&found.name)
                .map(|entry| entry.config.clone())
        };
        assert!(matches!(entry("zjson", "list-things"), Some(Ok(_))));
        let dotted = entry("elasticache", "describe-engine-default-parameters");
        let result_keys = dotted
            .and_then(std::result::Result::ok)
            .map(|config| config.result_key.iter().map(ToString::to_string).collect());
        assert_eq!(
            result_keys,
            Some(vec!["EngineDefaults.Parameters".to_owned()])
        );
        assert!(matches!(entry("kinesis", "describe-stream"), Some(Err(_))));
        assert_eq!(entry("zjson", "measure"), None);
    })
}

/// Only an epoch on the wire needs the local zone: json's default and an
/// explicit unixTimestamp; ISO 8601 and RFC 822 text, the query default,
/// carry their offset.
#[test]
fn only_epoch_timestamps_need_the_local_zone() -> Result<()> {
    with_models(|models| {
        for (command, operation, epoch) in [
            ("zjson", "stamp", false),
            ("zjson", "describe", true),
            ("zquery", "stamp", false),
            ("zquery", "describe", true),
        ] {
            let service = models
                .service(command)
                .unwrap_or_else(|| panic!("{command}"));
            let output = service
                .operation(operation)
                .and_then(|found| found.output)
                .unwrap_or_else(|| panic!("{command} {operation}"));
            assert_eq!(
                reaches_epoch_timestamp(&service.model, output),
                epoch,
                "{command} {operation}"
            );
        }
    })
}

/// Each refusal the client distinguishes keeps its own reason; none is
/// folded into another.
#[test]
fn each_client_refusal_names_its_own_reason() {
    let refusals = [
        Unsupported::Protocol("rest-xml".into()),
        Unsupported::Trait("httpChecksum".into()),
        Unsupported::OperationContextParams,
        Unsupported::Builtin("AWS::Example".into()),
        Unsupported::ClientContextParam("Example".into()),
        Unsupported::Auth("aws.auth#sigv4a".into()),
        Unsupported::UnsignedPayload,
        Unsupported::EndpointHeaders,
        Unsupported::RegionPattern,
        Unsupported::EndpointData,
        Unsupported::Validation(model::validate::Undecided::Comparison),
        Unsupported::Hook("declined".into()),
        Unsupported::StartingToken("a float"),
        Unsupported::Paginator(model::paginators::Refused::Expression(
            "Jobs[-1].JobId".into(),
        )),
    ];
    let reasons: Vec<&str> = refusals.iter().map(|one| reason(one).name()).collect();
    let mut distinct = reasons.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), reasons.len(), "{reasons:?}");
}

/// A blob payload streams at either end, marked `streaming` or not
/// (`_get_streaming_body`): apigateway's GetExport returns one and
/// ImportRestApi sends one; GetAccount has neither.
#[test]
fn a_blob_payload_at_either_end_is_streaming() -> Result<()> {
    with_models(|models| {
        let service = models
            .service("apigateway")
            .unwrap_or_else(|| panic!("apigateway"));
        let endpoints =
            EndpointData::load(models.search(), &service.name, service.model.metadata())
                .unwrap_or_else(|error| panic!("{error:?}"));
        for (operation, streams) in [
            ("get-export", true),
            ("import-rest-api", true),
            ("get-account", false),
        ] {
            let found = service
                .operation(operation)
                .unwrap_or_else(|| panic!("{operation}"));
            let refused = client::admission::static_refusal(
                &service.model,
                found,
                &endpoints,
                &service.name,
                &[],
                None,
            );
            assert_eq!(
                refused == Some(Unsupported::Trait("streaming".into())),
                streams,
                "{operation}: {refused:?}"
            );
        }
    })
}

/// Every shape id the arena hands out: the named shapes, their members,
/// list members, map keys and values, and the operations' references.
fn arena_ids(model: &Model) -> Vec<ShapeId> {
    let mut ids = Vec::new();
    for named in model.named_shapes() {
        ids.push(named);
        match model.shape(named).kind {
            ShapeKind::Structure { .. } => {
                ids.extend(model.members(named).iter().map(|member| member.shape));
            }
            ShapeKind::List { member } => ids.push(member),
            ShapeKind::Map { key, value } => ids.extend([key, value]),
            _ => {}
        }
    }
    for operation in model.operations() {
        ids.extend(operation.input.iter().chain(&operation.output));
        ids.extend(operation.errors.iter().copied());
    }
    ids
}

/// The schema of each arena shape reads as the arena does: its type, its
/// `timestampFormat` and its children.
fn check_schema_of(label: &str, model: &Model) -> std::result::Result<(), String> {
    for id in arena_ids(model) {
        let shape = model.shape(id);
        let schema =
            schema_of(model, id).ok_or_else(|| format!("{label}: {} unmapped", shape.name))?;
        let children = match shape.kind {
            ShapeKind::Structure { .. } => model.members(id).len(),
            ShapeKind::List { .. } => 1,
            ShapeKind::Map { .. } => 2,
            _ => 0,
        };
        let found = schema.members().len()
            + usize::from(schema.list_member().is_some())
            + 2 * usize::from(schema.map_key().is_some());
        let same = schema.type_name() == command::models::kind_name(&shape.kind)
            && schema.timestamp_format_text() == shape.traits.timestamp_format.as_deref()
            && found == children;
        if !same {
            return Err(format!("{label}: {} reads differently", shape.name));
        }
    }
    Ok(())
}

/// `schema_of` finds a schema for every arena shape of every shipped and
/// fake model, and the schema it finds reads as the arena shape does.
#[test]
fn every_arena_shape_has_the_schema_it_reads_as() -> Result<()> {
    use model::source::{DirSource, Kind, Source};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut count = 0;
    for dir in ["aws-cli/awscli/botocore/data", "tests/models"] {
        let source = DirSource::new(root.join(dir));
        for service in source.services(Kind::Service)? {
            let model = Model::load(&source, &service).map_err(|e| format!("{service}: {e}"))?;
            check_schema_of(&service, &model)?;
            count += 1;
        }
    }
    assert!(count > 400, "only {count} models read");
    Ok(())
}

/// A member's own `timestampFormat` decides, not its target's: the walk
/// reads the reference's schema, and an arena id of a reference maps to it.
#[test]
fn a_reference_s_timestamp_format_decides_the_local_zone() -> Result<()> {
    let model = Model::from_json(
        br#"{
          "metadata": {"protocol": "query"},
          "operations": {
            "Get": {"name": "Get", "output": {"shape": "Out"}},
            "Put": {"name": "Put", "output": {"shape": "Plain"}}
          },
          "shapes": {
            "Out": {"type": "structure", "members": {
              "Stamps": {"shape": "Stamps"}
            }},
            "Stamps": {"type": "list",
              "member": {"shape": "Time", "timestampFormat": "unixTimestamp"}},
            "Plain": {"type": "structure", "members": {"At": {"shape": "Time"}}},
            "Time": {"type": "timestamp"}
          }
        }"#,
    )?;
    let output = |name: &str| model.operation(name).and_then(|found| found.output);
    let (Some(get), Some(put)) = (output("Get"), output("Put")) else {
        panic!("outputs");
    };
    assert!(reaches_epoch_timestamp(&model, get));
    assert!(!reaches_epoch_timestamp(&model, put));
    let stamps = model.shape_named("Stamps").ok_or("Stamps")?;
    let ShapeKind::List { member } = model.shape(stamps).kind else {
        panic!("Stamps is a list");
    };
    assert!(reaches_epoch_timestamp(&model, member));
    let time = model.shape_named("Time").ok_or("Time")?;
    assert!(!reaches_epoch_timestamp(&model, time));
    check_schema_of("inline", &model).map_err(Into::into)
}
