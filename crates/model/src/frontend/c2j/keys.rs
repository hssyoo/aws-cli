//! The keys of a service description the C2J readers (the arena and the
//! C2J front-end) interpret, and the keys they may leave alone because no
//! code on botocore's call path reads them. Any other key is recorded as
//! [`Uninterpreted`], so a client can refuse an operation that reaches one
//! instead of silently ignoring what botocore would act on.
//!
//! The ignore lists hold only keys the vendored botocore never reads, or
//! reads for documentation alone (`documentation`, `examples`), checked
//! against its source by this module's tests. A key that botocore's
//! serializers, parsers, validator, signer, retry or endpoint code reads is
//! never ignored: it is interpreted, or it is recorded.
use super::traits::{is_trait_key, trait_subkeys};
use crate::schema::{Site, Uninterpreted};
use serde_json::{Map, Value as Json};

type Object = Map<String, Json>;

/// Top-level keys of `service-2.json` the arena reads.
pub(crate) const DOCUMENT: &[&str] = &["metadata", "operations", "shapes", "clientContextParams"];
/// Top-level keys no call reads: `documentation` (`ServiceModel.
/// documentation`, for docs), `version` (the description's format, which
/// only `configure add-model` checks), `examples` (`docs/service.py`), and
/// a deprecated service's `deprecated` and `deprecatedMessage`, which
/// botocore does not read at this level.
pub(crate) const DOCUMENT_IGNORED: &[&str] = &[
    "documentation",
    "version",
    "examples",
    "deprecated",
    "deprecatedMessage",
];

/// `metadata` keys the arena reads (`ServiceModel.metadata` and the
/// attributes over it).
pub(crate) const METADATA: &[&str] = &[
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
pub(crate) const METADATA_IGNORED: &[&str] = &[
    "serviceFullName",
    "serviceAbbreviation",
    "protocolSettings",
    "ripServiceName",
    "globalEndpoint",
    "checksumFormat",
];

/// Operation keys the arena reads (`OperationModel`).
pub(crate) const OPERATION: &[&str] = &[
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
pub(crate) const OPERATION_IGNORED: &[&str] = &[
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
pub(crate) const OPERATION_NESTED: &[(&str, &[&str], &[&str])] = &[
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
pub(crate) const HTTP_IGNORED: &[&str] = &["responseCode"];

/// Shape keys the arena reads besides its traits: the type and children.
pub(crate) const SHAPE: &[&str] = &["type", "members", "member", "key", "value"];
/// Shape keys no call reads: `documentation` (`Shape.documentation`, for
/// docs) and `deprecatedMessage`, `deprecatedSince`, `box`, `fault`,
/// `wrapper`, `synthetic`, `sparse` and `xmlOrder`, which botocore never
/// reads.
pub(crate) const SHAPE_IGNORED: &[&str] = &[
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
pub(crate) const REFERENCE_IGNORED: &[&str] = &[
    "documentation",
    "deprecatedMessage",
    "deprecatedSince",
    "box",
];

/// Keys of a `clientContextParams` entry: `type` is read, `documentation`
/// is for docs.
pub(crate) const CLIENT_CONTEXT_PARAM: &[&str] = &["type"];
pub(crate) const CLIENT_CONTEXT_PARAM_IGNORED: &[&str] = &["documentation"];

/// Where a recorded key goes: its site and the path before the key. It is
/// built only when a key is recorded, so the common, clean object costs no
/// allocation.
pub(crate) type At<'a> = &'a dyn Fn() -> (Site, String);

/// Records every key of `object` neither `known` nor `ignored`, each as
/// [`At`]'s path and the key.
pub(crate) fn record(
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
pub(crate) fn record_traits(
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
/// gives the `shape` value and whether any trait key is present, which
/// `ShapeResolver.resolve_shape_ref` lays over the target's.
pub(crate) fn scan_reference<'a>(
    object: &'a Object,
    at: At<'_>,
    out: &mut Vec<Uninterpreted>,
) -> (Option<&'a Json>, bool) {
    let (mut shape, mut traits) = (None, false);
    for (key, value) in object {
        let key = key.as_str();
        if key == "shape" {
            shape = Some(value);
            continue;
        }
        if REFERENCE_IGNORED.contains(&key) {
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
    (shape, traits)
}

/// Records an operation's keys and its nested objects' that no reader
/// interprets; its references record theirs as they resolve.
pub(crate) fn record_operation(name: &str, object: &Object, out: &mut Vec<Uninterpreted>) {
    // Each path is formatted only when a key is recorded.
    let site = || Site::Operation(name.into());
    record(
        object,
        OPERATION,
        OPERATION_IGNORED,
        &|| (site(), String::new()),
        out,
    );
    for (key, known, ignored) in OPERATION_NESTED {
        if let Some(nested) = super::get(object, key).and_then(Json::as_object) {
            record(nested, known, ignored, &|| (site(), format!("{key}.")), out);
        }
    }
    let params = [
        ("staticContextParams", "value"),
        ("operationContextParams", "path"),
    ];
    for (key, known) in params {
        let entries = super::get(object, key)
            .and_then(Json::as_object)
            .into_iter()
            .flatten();
        for (param, props) in entries {
            if let Some(props) = props.as_object() {
                let at = || (site(), format!("{key}.{param}."));
                record(props, &[known], &[], &at, out);
            }
        }
    }
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
