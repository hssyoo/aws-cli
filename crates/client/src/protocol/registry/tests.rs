use super::*;
use crate::error::{ParseError, SerializeError};
use crate::http::HttpResponse;
use crate::protocol::{IMPLEMENTED, ParseOptions, SWITCHED_OFF, Serialized, compat, for_model};
use model::schema::{OperationSchema, Schema};
use model::shapes::Model;
use model::source::{DirSource, Kind, Source};
use model::value::{Map, Value};

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

/// A protocol that is implemented, for the positive path.
#[derive(Debug)]
struct Fake(TraitId);

impl ClientProtocol for Fake {
    fn protocol_id(&self) -> TraitId {
        self.0
    }
    fn payload_codec(&self) -> Option<&dyn model::serde::Codec> {
        None
    }
    fn compat(&self) -> &crate::protocol::BotocoreCompat {
        &crate::protocol::BotocoreCompat::RPC
    }
    fn serialize_request(
        &self,
        _operation: OperationSchema<'_>,
        _input: &Value,
        _model: &Model,
    ) -> Result<Serialized, SerializeError> {
        Err(SerializeError::new("fake"))
    }
    fn deserialize_response(
        &self,
        _operation: OperationSchema<'_>,
        _shape: Option<Schema<'_>>,
        _response: &HttpResponse,
        _options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        Err(ParseError::new("fake"))
    }
    fn parse_error_metadata(&self, _response: &HttpResponse) -> Result<Map, ParseError> {
        Err(ParseError::new("fake"))
    }
    fn deserialize_modeled_error(
        &self,
        _shape: Schema<'_>,
        _response: &HttpResponse,
        _options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        Err(ParseError::new("fake"))
    }
}

static QUERY: Fake = Fake(traits::AWS_QUERY);
static JSON: Fake = Fake(traits::AWS_JSON_1_1);

type Chosen = Result<TraitId, Unsupported>;

fn chosen(registry: &ProtocolRegistry, metadata: &Metadata) -> Chosen {
    registry
        .resolve(metadata)
        .map(|protocol| protocol.protocol_id())
}

/// The implementation is the resolved protocol's, and none when that one
/// is not implemented, even if another the model lists is: botocore would
/// speak the resolved one.
#[test]
fn an_implementation_is_chosen_for_the_resolved_protocol_only() {
    static IMPLEMENTED: ProtocolRegistry =
        ProtocolRegistry::new(&[("query", &QUERY), ("json", &JSON)]);
    static ONLY_QUERY: ProtocolRegistry = ProtocolRegistry::new(&[("query", &QUERY)]);
    let (implemented, only_query) = (&IMPLEMENTED, &ONLY_QUERY);
    let json_first = metadata(Some("query"), Some(&["query", "json"]));
    assert_eq!(chosen(implemented, &json_first), Ok(traits::AWS_JSON_1_1));
    assert_eq!(
        chosen(only_query, &json_first),
        Err(Unsupported::Protocol("json".into()))
    );
    let query = metadata(Some("query"), None);
    assert_eq!(chosen(only_query, &query), Ok(traits::AWS_QUERY));
    assert_eq!(
        chosen(implemented, &metadata(None, None)),
        Err(Unsupported::Protocol(String::new()))
    );
    let unlisted = metadata(Some("json"), Some(&["smithy-rpc-v3"]));
    assert_eq!(
        chosen(implemented, &unlisted),
        Err(Unsupported::Protocol(String::new()))
    );
}

/// botocore's `json` is awsJson 1.0 or 1.1 by `jsonVersion`; without a
/// version it is the name's first instance, whose serializer then refuses
/// the missing `jsonVersion` as before.
#[test]
fn the_json_version_tells_the_aws_json_instances_apart() {
    let json = |version: Option<&str>| Metadata {
        json_version: version.map(Into::into),
        ..metadata(Some("json"), None)
    };
    for (version, id) in [
        (Some("1.0"), traits::AWS_JSON_1_0),
        (Some("1.1"), traits::AWS_JSON_1_1),
        (Some("2.0"), traits::AWS_JSON_1_1),
        (None, traits::AWS_JSON_1_1),
    ] {
        assert_eq!(chosen(&IMPLEMENTED, &json(version)), Ok(id), "{version:?}");
    }
}

/// Each implementation answers to its botocore name and its protocol
/// trait, and only the JSON ones send epoch timestamps; nothing is left
/// switched off.
#[test]
fn the_registry_holds_each_protocol_once_by_trait() {
    for (name, id, epoch) in [
        ("query", traits::AWS_QUERY, false),
        ("json", traits::AWS_JSON_1_1, true),
        ("rest-json", traits::REST_JSON_1, true),
        ("rest-xml", traits::REST_XML, false),
        ("ec2", traits::EC2_QUERY, false),
    ] {
        let named = IMPLEMENTED
            .named(name)
            .map(|protocol| protocol.protocol_id());
        assert_eq!(named, Some(id), "{name}");
        let by_id = IMPLEMENTED.get(id).map(compat::timestamps_epoch);
        assert_eq!(by_id, Some(epoch), "{name}");
    }
    let json_1_0 = IMPLEMENTED.get(traits::AWS_JSON_1_0);
    assert_eq!(json_1_0.map(compat::timestamps_epoch), Some(true));
    for name in ["rest-xml", "ec2"] {
        assert!(SWITCHED_OFF.named(name).is_none(), "{name}");
    }
}

/// Every model in the repository's botocore data resolves to a protocol,
/// and exactly those resolving to `query`, `json`, `rest-json`, `rest-xml`
/// or `ec2` have an implementation.
#[test]
#[cfg_attr(miri, ignore = "reads the data directory, which Miri cannot")]
fn every_model_but_cbor_has_an_implementation() -> TestResult {
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
        let expected = matches!(
            resolved,
            Ok("query" | "json" | "rest-json" | "rest-xml" | "ec2")
        );
        assert_eq!(for_model(&model).is_some(), expected, "{service}");
        implemented += usize::from(expected);
    }
    assert!(implemented > 100, "{implemented} implemented");
    Ok(())
}
