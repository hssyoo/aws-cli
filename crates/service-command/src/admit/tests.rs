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
        let source = ServiceSource::new(models, everything);
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
        let source = ServiceSource::new(models, everything);
        let refused = |customizations: &[&Customization], argv: &[&str]| {
            admitted(&source, customizations, argv).err()
        };
        assert_eq!(
            refused(&[], &["zquery", "get-thing", "--thing-id", "a"]),
            Some(PythonReason::UnportedCustomization)
        );
        assert_eq!(
            refused(everything, &["zjson", "list-things"]),
            Some(PythonReason::UnsupportedTrait)
        );
        assert_eq!(
            refused(&[], &["lambda", "list-functions"]),
            Some(PythonReason::UnsupportedProtocol)
        );
    })
}
