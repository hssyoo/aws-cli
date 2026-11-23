//! The keys of a service description the arena interprets, and the keys it
//! may leave alone because no code on botocore's call path reads them. Any
//! other key is recorded as [`Uninterpreted`], so a client can refuse an
//! operation that reaches one instead of silently ignoring what botocore
//! would act on.
//!
//! The ignore lists hold only keys the vendored botocore never reads, or
//! reads for documentation alone (`documentation`, `examples`), checked
//! against its source by this module's tests. A key that botocore's
//! serializers, parsers, validator, signer, retry or endpoint code reads is
//! never ignored: it is interpreted, or it is recorded.
use super::traits::{is_trait_key, trait_subkeys};
use super::{Site, Uninterpreted};
use serde_json::{Map, Value as Json};

type Object = Map<String, Json>;

/// Top-level keys of `service-2.json` the arena reads.
pub(super) const DOCUMENT: &[&str] = &["metadata", "operations", "shapes", "clientContextParams"];
/// Top-level keys no call reads: `documentation` (`ServiceModel.
/// documentation`, for docs), `version` (the description's format, which
/// only `configure add-model` checks), `examples` (`docs/service.py`), and
/// a deprecated service's `deprecated` and `deprecatedMessage`, which
/// botocore does not read at this level.
pub(super) const DOCUMENT_IGNORED: &[&str] = &[
    "documentation",
    "version",
    "examples",
    "deprecated",
    "deprecatedMessage",
];

/// `metadata` keys the arena reads (`ServiceModel.metadata` and the
/// attributes over it).
pub(super) const METADATA: &[&str] = &[
    "apiVersion",
    "endpointPrefix",
    "protocol",
    "protocols",
    "jsonVersion",
    "targetPrefix",
    "serviceId",
    "signatureVersion",
    "signingName",
    "uid",
    "xmlNamespace",
    "auth",
    "awsQueryCompatible",
];
/// `metadata` keys no call reads: `serviceFullName` and
/// `serviceAbbreviation`, which botocore reads only to name a client's
/// Python class (`utils.get_service_module_name`), and `protocolSettings`
/// (an HTTP/2 hint for event streams), `ripServiceName`, `globalEndpoint`
/// and `checksumFormat`, which the vendored botocore never reads (a global
/// endpoint comes from the rule set).
pub(super) const METADATA_IGNORED: &[&str] = &[
    "serviceFullName",
    "serviceAbbreviation",
    "protocolSettings",
    "ripServiceName",
    "globalEndpoint",
    "checksumFormat",
];

/// Operation keys the arena reads (`OperationModel`).
pub(super) const OPERATION: &[&str] = &[
    "name",
    "http",
    "input",
    "output",
    "errors",
    "endpointdiscovery",
    "endpointoperation",
    "auth",
    "authtype",
    "unsignedPayload",
    "httpChecksumRequired",
    "httpChecksum",
    "requestcompression",
    "staticContextParams",
    "operationContextParams",
    "endpoint",
];
/// Operation keys no call reads: `documentation` (`OperationModel.
/// documentation`, for docs), `deprecated` (`OperationModel.deprecated`,
/// which only awscli reads, to leave the command out of help:
/// `ServiceOperation._UNDOCUMENTED`), and `deprecatedMessage`,
/// `deprecatedSince`, `idempotent` and `readonly`, which botocore never
/// reads.
pub(super) const OPERATION_IGNORED: &[&str] = &[
    "documentation",
    "deprecated",
    "deprecatedMessage",
    "deprecatedSince",
    "idempotent",
    "readonly",
];

/// The keys of an operation's nested objects the arena reads, and those
/// no call reads: `http.responseCode`, which botocore's parsers never
/// read (they take the response's status).
pub(super) const OPERATION_NESTED: &[(&str, &[&str], &[&str])] = &[
    ("http", &["method", "requestUri"], HTTP_IGNORED),
    ("endpoint", &["hostPrefix"], &[]),
    ("endpointdiscovery", &["required"], &[]),
    (
        "httpChecksum",
        &[
            "requestAlgorithmMember",
            "requestChecksumRequired",
            "requestValidationModeMember",
            "responseAlgorithms",
        ],
        &[],
    ),
    ("requestcompression", &["encodings"], &[]),
];
pub(super) const HTTP_IGNORED: &[&str] = &["responseCode"];

/// Shape keys the arena reads besides its traits: the type and children.
pub(super) const SHAPE: &[&str] = &["type", "members", "member", "key", "value"];
/// Shape keys no call reads: `documentation` (`Shape.documentation`, for
/// docs) and `deprecatedMessage`, `deprecatedSince`, `box`, `fault`,
/// `wrapper`, `synthetic`, `sparse` and `xmlOrder`, which botocore never
/// reads.
pub(super) const SHAPE_IGNORED: &[&str] = &[
    "documentation",
    "deprecatedMessage",
    "deprecatedSince",
    "box",
    "fault",
    "wrapper",
    "synthetic",
    "sparse",
    "xmlOrder",
];
/// Keys of a shape reference (a member, a list member, a map key or value,
/// an operation's input, output or error) that no call reads, as for
/// [`SHAPE_IGNORED`]; `shape` itself is read.
pub(super) const REFERENCE_IGNORED: &[&str] = &[
    "documentation",
    "deprecatedMessage",
    "deprecatedSince",
    "box",
];

/// Keys of a `clientContextParams` entry: `type` is read, `documentation`
/// is for docs.
pub(super) const CLIENT_CONTEXT_PARAM: &[&str] = &["type"];
pub(super) const CLIENT_CONTEXT_PARAM_IGNORED: &[&str] = &["documentation"];

/// Where a recorded key goes: its site and the path before the key. It is
/// built only when a key is recorded, so the common, clean object costs no
/// allocation.
pub(super) type At<'a> = &'a dyn Fn() -> (Site, String);

/// Records every key of `object` neither `known` nor `ignored`, each as
/// [`At`]'s path and the key.
pub(super) fn record(
    object: &Object,
    known: &[&str],
    ignored: &[&str],
    at: At<'_>,
    out: &mut Vec<Uninterpreted>,
) {
    for key in object.keys() {
        if !known.contains(&key.as_str()) && !ignored.contains(&key.as_str()) {
            push(at, key, out);
        }
    }
}

/// A shape or a shape reference: its traits (with their nested objects),
/// `extra` keys, and the ignored ones.
pub(super) fn record_traits(
    object: &Object,
    extra: &[&str],
    ignored: &[&str],
    at: At<'_>,
    out: &mut Vec<Uninterpreted>,
) {
    for (key, value) in object {
        let key = key.as_str();
        if extra.contains(&key) || ignored.contains(&key) {
            continue;
        }
        if !is_trait_key(key) {
            push(at, key, out);
        } else if let (Some(known), Some(nested)) = (trait_subkeys(key), value.as_object()) {
            let nested_at = || {
                let (site, prefix) = at();
                (site, format!("{prefix}{key}."))
            };
            record(nested, known, &[], &nested_at, out);
        }
    }
}

/// One pass over a shape reference's keys: records those that are neither
/// `shape`, a trait nor ignored (and a trait object's unread subkeys), and
/// says whether any trait key is present, which `ShapeResolver.
/// resolve_shape_ref` lays over the target's.
pub(super) fn scan_reference(object: &Object, at: At<'_>, out: &mut Vec<Uninterpreted>) -> bool {
    let mut traits = false;
    for (key, value) in object {
        let key = key.as_str();
        if key == "shape" || REFERENCE_IGNORED.contains(&key) {
            continue;
        }
        if !is_trait_key(key) {
            push(at, key, out);
            continue;
        }
        traits = true;
        if let (Some(known), Some(nested)) = (trait_subkeys(key), value.as_object()) {
            let nested_at = || {
                let (site, prefix) = at();
                (site, format!("{prefix}{key}."))
            };
            record(nested, known, &[], &nested_at, out);
        }
    }
    traits
}

fn push(at: At<'_>, key: &str, out: &mut Vec<Uninterpreted>) {
    let (site, prefix) = at();
    out.push(Uninterpreted {
        site,
        key: format!("{prefix}{key}").into(),
    });
}

#[cfg(test)]
mod tests;
