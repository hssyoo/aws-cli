#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::ServiceSource;
use crate::testing::{Result, parsed, replacing_all, with_models};

fn admitted(
    source: &ServiceSource<'_>,
    customizations: &[&Customization],
    argv: &[&str],
) -> std::result::Result<Admitted, PythonReason> {
    admit(source.models(), customizations, &parsed(source, argv))
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
/// paginator whose expression the native paginator does not run (kinesis's
/// `StreamDescription.Shards[-1].ShardId`), an unimplemented protocol.
#[test]
fn each_stage_names_its_refusal() -> Result<()> {
    with_models(|models| {
        let everything = replacing_all(&models, "kinesis", "describe-stream");
        let source = crate::testing::source(models, everything);
        let refused = |customizations: &[&Customization], argv: &[&str]| {
            admitted(&source, customizations, argv).err()
        };
        assert_eq!(
            refused(&[], &["zquery", "get-thing", "--thing-id", "a"]),
            Some(PythonReason::UnportedCustomization)
        );
        assert_eq!(
            refused(
                everything,
                &["kinesis", "describe-stream", "--stream-name", "s"]
            ),
            Some(PythonReason::UnsupportedTrait)
        );
        assert_eq!(
            refused(&[], &["s3api", "list-buckets"]),
            Some(PythonReason::UnsupportedProtocol)
        );
    })
}

/// A paginated operation is admitted with its paginator when paging is on,
/// and as one call under `--no-paginate`.
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
        assert_eq!((found.raised, found.warned), (None, false));
        let single = admitted(
            &source,
            everything,
            &["zjson", "list-things", "--no-paginate"],
        )
        .unwrap_or_else(|reason| panic!("{reason:?}"));
        assert_eq!(single.paginated, None);
        let raised = admitted(
            &source,
            everything,
            &["zjson", "list-things", "--no-paginate", "--max-items", "2"],
        )
        .unwrap_or_else(|reason| panic!("{reason:?}"));
        assert!(raised.raised.is_some());
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
            paginator(service, found)
        };
        assert!(matches!(entry("zjson", "list-things"), Some(Ok(_))));
        let dotted = entry("elasticache", "describe-engine-default-parameters");
        let result_keys = dotted
            .and_then(std::result::Result::ok)
            .map(|config| config.result_key);
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

/// A blob payload streams at either end, marked `streaming` or not
/// (`_get_streaming_body`): apigateway's GetExport returns one and
/// ImportRestApi sends one; GetAccount has neither.
#[test]
fn a_blob_payload_at_either_end_is_streaming() -> Result<()> {
    with_models(|models| {
        let service = models
            .service("apigateway")
            .unwrap_or_else(|| panic!("apigateway"));
        for (operation, streams) in [
            ("get-export", true),
            ("import-rest-api", true),
            ("get-account", false),
        ] {
            let found = service
                .operation(operation)
                .unwrap_or_else(|| panic!("{operation}"));
            assert_eq!(unimplemented_trait(service, found), streams, "{operation}");
        }
    })
}
