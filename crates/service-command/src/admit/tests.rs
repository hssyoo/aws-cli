#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::ServiceSource;
use crate::testing::{Result, parsed, replacing_all, with_models};

fn admitted(
    source: &ServiceSource<'_>,
    customizations: &[&'static Customization],
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
/// paginated operation, an unimplemented protocol.
#[test]
fn each_stage_names_its_refusal() -> Result<()> {
    with_models(|models| {
        let everything = replacing_all(&models, "zjson", "list-things");
        let source = crate::testing::source(models, everything);
        let refused = |customizations: &[&'static Customization], argv: &[&str]| {
            admitted(&source, customizations, argv).err()
        };
        assert_eq!(
            refused(&[], &["zquery", "get-thing", "--thing-id", "a"]),
            Some(PythonReason::UnportedCustomization)
        );
        assert_eq!(
            refused(everything, &["zjson", "list-things"]),
            Some(PythonReason::Paginator)
        );
        assert_eq!(
            refused(&[], &["s3api", "list-buckets"]),
            Some(PythonReason::UnsupportedProtocol)
        );
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
            );
            assert_eq!(
                refused == Some(Unsupported::Trait("streaming".into())),
                streams,
                "{operation}: {refused:?}"
            );
        }
    })
}
