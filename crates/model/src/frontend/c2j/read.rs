//! Typed reads of the document's sections, `metadata`, `clientContextParams`
//! and each operation's own keys, with the arena's errors in its order.
use super::keys;
use super::traits::truthy;
use crate::schema::{
    ClientContextParam, EndpointDiscovery, HttpChecksum, OperationData, ServiceData, Site, TraitId,
    Uninterpreted, traits,
};
use crate::shapes::ModelError;
use crate::value::Value;
use serde_json::{Map, Value as Json};

type Object = Map<String, Json>;
type Texts = Option<Box<[Box<str>]>>;
type Params = Box<[(Box<str>, Value)]>;
type Paths = Box<[(Box<str>, Box<str>)]>;

/// A top-level object; absent reads as empty, as `ServiceModel` reads it.
pub(super) fn section<'a>(
    document: &'a Object,
    key: &'static str,
) -> Result<&'a Object, ModelError> {
    match document.get(key) {
        None => Ok(&super::EMPTY),
        Some(value) => value.as_object().ok_or(ModelError::InvalidTrait {
            shape: "(model)".into(),
            name: key,
        }),
    }
}

/// Typed reads of an operation's or the metadata's keys.
struct Fields<'a> {
    object: &'a Object,
    owner: &'a str,
}

impl Fields<'_> {
    fn invalid(&self, name: &'static str) -> ModelError {
        ModelError::InvalidTrait {
            shape: self.owner.into(),
            name,
        }
    }

    fn flag(&self, key: &str) -> bool {
        super::get(self.object, key).is_some_and(truthy)
    }

    fn text(&self, key: &'static str) -> Result<Option<Box<str>>, ModelError> {
        super::get(self.object, key)
            .map(|value| {
                value
                    .as_str()
                    .map(Box::from)
                    .ok_or_else(|| self.invalid(key))
            })
            .transpose()
    }

    fn texts(&self, key: &'static str) -> Result<Texts, ModelError> {
        super::get(self.object, key)
            .map(|value| strings(value).ok_or_else(|| self.invalid(key)))
            .transpose()
    }

    fn nested(&self, key: &'static str) -> Result<Option<Fields<'_>>, ModelError> {
        let Some(value) = super::get(self.object, key) else {
            return Ok(None);
        };
        let object = value.as_object().ok_or_else(|| self.invalid(key))?;
        Ok(Some(Fields {
            object,
            owner: self.owner,
        }))
    }
}

/// A JSON list of strings.
fn strings(value: &Json) -> Option<Box<[Box<str>]>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(Box::from))
        .collect()
}

/// The service's `metadata`: what the schema holds of it, every key read
/// with its type checked in the arena's order (the protocol, JSON version,
/// target prefix, signature version and uid are botocore's alone).
pub(super) fn metadata(
    metadata: Option<&Json>,
    out: &mut Vec<Uninterpreted>,
) -> Result<ServiceData, ModelError> {
    let object = match metadata {
        None => &*super::EMPTY,
        Some(value) => value.as_object().ok_or(ModelError::InvalidTrait {
            shape: "(model)".into(),
            name: "metadata",
        })?,
    };
    let at = || (Site::Service, "metadata.".to_owned());
    keys::record(object, keys::METADATA, keys::METADATA_IGNORED, &at, out);
    let read = Fields {
        object,
        owner: "metadata",
    };
    let version = read.text("apiVersion")?;
    let endpoint_prefix = read.text("endpointPrefix")?;
    let protocol = read.text("protocol")?;
    let protocols = read.texts("protocols")?;
    let json_version = read.text("jsonVersion")?;
    read.text("targetPrefix")?;
    let sdk_id = read.text("serviceId")?;
    read.text("signatureVersion")?;
    let sigv4_name = read.text("signingName")?;
    read.text("uid")?;
    Ok(ServiceData {
        version,
        sdk_id,
        endpoint_prefix,
        sigv4_name,
        xml_namespace: read.text("xmlNamespace")?,
        auth: read.texts("auth")?,
        query_compatible: object.contains_key("awsQueryCompatible"),
        protocol_traits: protocol_traits(protocol, protocols, json_version.as_deref()),
        client_context_params: Box::default(),
    })
}

/// The protocol traits `protocol` and `protocols` name, as a set in
/// [`TraitId`] order; a name with no trait adds none.
pub(super) fn protocol_traits(
    protocol: Option<Box<str>>,
    protocols: Texts,
    json_version: Option<&str>,
) -> Box<[TraitId]> {
    let names = protocols.into_iter().flatten().chain(protocol);
    let mut ids: Vec<TraitId> = names
        .filter_map(|name| match (&*name, json_version) {
            ("json", Some("1.0")) => Some(traits::AWS_JSON_1_0),
            ("json", Some("1.1")) => Some(traits::AWS_JSON_1_1),
            ("rest-json", _) => Some(traits::REST_JSON_1),
            ("rest-xml", _) => Some(traits::REST_XML),
            ("query", _) => Some(traits::AWS_QUERY),
            ("ec2", _) => Some(traits::EC2_QUERY),
            ("smithy-rpc-v2-cbor", _) => Some(traits::RPC_V2_CBOR),
            _ => None,
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids.into()
}

/// `ServiceModel.client_context_parameters`.
pub(super) fn client_context_params(
    document: &Object,
    out: &mut Vec<Uninterpreted>,
) -> Result<Box<[ClientContextParam]>, ModelError> {
    let params = section(document, "clientContextParams")?;
    params
        .iter()
        .map(|(name, param)| {
            if let Some(object) = param.as_object() {
                let prefix = format!("clientContextParams.{name}.");
                let (known, ignored) = (
                    keys::CLIENT_CONTEXT_PARAM,
                    keys::CLIENT_CONTEXT_PARAM_IGNORED,
                );
                keys::record(
                    object,
                    known,
                    ignored,
                    &|| (Site::Service, prefix.clone()),
                    out,
                );
            }
            let type_name = param.get("type").and_then(Json::as_str);
            let type_name = type_name.ok_or(ModelError::InvalidTrait {
                shape: name.as_str().into(),
                name: "clientContextParams",
            })?;
            Ok(ClientContextParam {
                name: name.as_str().into(),
                type_name: type_name.into(),
            })
        })
        .collect()
}

/// An operation's own keys, as `OperationModel` reads them, in the
/// arena's order.
pub(super) fn operation(name: &str, object: &Object) -> Result<OperationData, ModelError> {
    let read = Fields {
        object,
        owner: name,
    };
    let wire_name = read.text("name")?;
    let (http_method, http_request_uri) = match read.nested("http")? {
        Some(http) => (http.text("method")?, http.text("requestUri")?),
        None => (None, None),
    };
    let endpoint_discovery = read
        .nested("endpointdiscovery")?
        .map(|discovery| EndpointDiscovery {
            required: discovery.flag("required"),
        });
    Ok(OperationData {
        name: name.into(),
        wire_name,
        http_method,
        http_request_uri,
        endpoint_discovery,
        endpoint_operation: read.flag("endpointoperation"),
        auth: read.texts("auth")?,
        auth_type: read.text("authtype")?,
        unsigned_payload: read.flag("unsignedPayload"),
        http_checksum_required: read.flag("httpChecksumRequired"),
        http_checksum: http_checksum(&read)?,
        request_compression: match read.nested("requestcompression")? {
            Some(compression) => Some(compression.texts("encodings")?.unwrap_or_default()),
            None => None,
        },
        static_context_params: static_context_params(&read)?,
        operation_context_params: operation_context_params(&read)?,
        host_prefix: match read.nested("endpoint")? {
            Some(endpoint) => endpoint.text("hostPrefix")?,
            None => None,
        },
    })
}

fn http_checksum(read: &Fields<'_>) -> Result<Option<HttpChecksum>, ModelError> {
    let Some(checksum) = read.nested("httpChecksum")? else {
        return Ok(None);
    };
    Ok(Some(HttpChecksum {
        request_algorithm_member: checksum.text("requestAlgorithmMember")?,
        request_checksum_required: checksum.flag("requestChecksumRequired"),
        request_validation_mode_member: checksum.text("requestValidationModeMember")?,
        response_algorithms: checksum.texts("responseAlgorithms")?.unwrap_or_default(),
    }))
}

fn static_context_params(read: &Fields<'_>) -> Result<Params, ModelError> {
    let Some(params) = read.nested("staticContextParams")? else {
        return Ok(Box::default());
    };
    let value = |props: &Json| props.get("value").map_or(Value::Null, Value::from_serde);
    Ok(params
        .object
        .iter()
        .map(|(name, props)| (name.as_str().into(), value(props)))
        .collect())
}

fn operation_context_params(read: &Fields<'_>) -> Result<Paths, ModelError> {
    let Some(params) = read.nested("operationContextParams")? else {
        return Ok(Box::default());
    };
    params
        .object
        .iter()
        .map(|(name, props)| {
            let path = props.get("path").and_then(Json::as_str);
            let path = path.ok_or_else(|| read.invalid("operationContextParams"))?;
            Ok((name.as_str().into(), path.into()))
        })
        .collect()
}
