use super::*;
use crate::error::ServiceError;
use crate::http::HttpResponse;
use model::source::{DirSource, Kind, Source};
use model::value::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn metadata(protocol: Option<&str>, protocols: Option<&[&str]>) -> Metadata {
    Metadata {
        protocol: protocol.map(Into::into),
        protocols: protocols.map(|names| names.iter().map(|name| (*name).into()).collect()),
        ..Metadata::default()
    }
}

/// botocore's `ServiceModel.resolved_protocol`: the first of
/// `PRIORITY_ORDERED_SUPPORTED_PROTOCOLS` that `protocols` lists, whatever
/// order the model lists them in; `protocol` only when `protocols` is
/// absent or empty.
#[test]
fn the_resolved_protocol_is_botocore_s_priority_choice() {
    for (protocol, protocols, resolved) in [
        (Some("query"), None, Ok("query")),
        (Some("json"), Some(&["query", "json"][..]), Ok("json")),
        (
            Some("query"),
            Some(&["smithy-rpc-v2-cbor", "query"][..]),
            Ok("smithy-rpc-v2-cbor"),
        ),
        (Some("ec2"), Some(&["ec2", "query"][..]), Ok("query")),
        (Some("rest-xml"), Some(&[][..]), Ok("rest-xml")),
        (
            Some("json"),
            Some(&["smithy-rpc-v3"][..]),
            Err(Unresolved::Unsupported(vec!["smithy-rpc-v3".into()])),
        ),
        (None, None, Err(Unresolved::Missing)),
    ] {
        let metadata = metadata(protocol, protocols);
        assert_eq!(
            resolved_protocol(&metadata),
            resolved,
            "{protocol:?} {protocols:?}"
        );
    }
}

/// A protocol that is implemented, for the positive path: today none is.
#[derive(Debug)]
struct Fake(&'static str);

impl Protocol for Fake {
    fn name(&self) -> &'static str {
        self.0
    }
    fn epoch_timestamps(&self) -> bool {
        false
    }
    fn serialize(
        &self,
        _model: &Model,
        _operation: &Operation,
        _input: &Value,
    ) -> Result<Serialized, SerializeError> {
        Err(SerializeError::new("fake"))
    }
    fn parse(
        &self,
        _model: &Model,
        _operation: &Operation,
        _response: &HttpResponse,
        _options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        Err(ParseError::new("fake"))
    }
    fn parse_error(
        &self,
        _model: &Model,
        operation: &Operation,
        response: &HttpResponse,
        _options: &ParseOptions<'_>,
    ) -> Result<ServiceError, ParseError> {
        Ok(ServiceError::new(&operation.name, response.status))
    }
}

static QUERY: Fake = Fake("query");
static JSON: Fake = Fake("json");

/// The implementation is the resolved protocol's, and none when that one
/// is not implemented, even if another the model lists is: botocore would
/// speak the resolved one.
#[test]
fn an_implementation_is_chosen_for_the_resolved_protocol_only() {
    let implemented: &[&'static dyn Protocol] = &[&QUERY, &JSON];
    let only_query: &[&'static dyn Protocol] = &[&QUERY];
    let name = |chosen: Option<&dyn Protocol>| chosen.map(|protocol| protocol.name());
    let json_first = metadata(Some("query"), Some(&["query", "json"]));
    assert_eq!(name(select(&json_first, implemented)), Some("json"));
    assert_eq!(name(select(&json_first, only_query)), None);
    let query = metadata(Some("query"), None);
    assert_eq!(name(select(&query, only_query)), Some("query"));
    assert_eq!(name(select(&metadata(None, None), implemented)), None);
}

/// Every model in the repository's botocore data resolves to a protocol,
/// and exactly those resolving to `query`, `json` or `rest-json` have an
/// implementation.
#[test]
#[cfg_attr(miri, ignore = "reads the data directory, which Miri cannot")]
fn query_json_and_rest_json_models_have_an_implementation() -> TestResult {
    let data =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/awscli/botocore/data");
    let source = DirSource::new(data);
    let services = source.services(Kind::Service)?;
    assert!(services.len() > 400, "{} services", services.len());
    let mut implemented = 0;
    for service in services {
        let model = Model::load(&source, &service)?;
        let resolved = resolved_protocol(model.metadata());
        assert!(resolved.is_ok(), "{service}: {resolved:?}");
        let expected = matches!(resolved, Ok("query" | "json" | "rest-json"));
        assert_eq!(for_model(&model).is_some(), expected, "{service}");
        implemented += usize::from(expected);
    }
    assert!(implemented > 100, "{implemented} implemented");
    Ok(())
}
