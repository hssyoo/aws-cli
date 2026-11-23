//! The bodies of the fuzz targets over this crate's readers
//! (`fuzz/fuzz_targets/xml_reader.rs`, `responses.rs`), which the stable
//! replay test (its `tests`) also runs. No input may panic.
use crate::http::{Headers, HttpResponse};
use crate::error::ParseError;
use crate::protocol::{self, ParseOptions};
use model::shapes::Model;
use model::value::Value;

/// `ElementTree.XMLParser` over any bytes: a tree or expat's error. A tree
/// reads the same twice.
pub fn xml(bytes: &[u8]) {
    let first = crate::xml::parse(bytes);
    assert_eq!(first.is_ok(), crate::xml::parse(bytes).is_ok());
}

/// Statuses a response may carry: success, the redirects the error path
/// starts at, client and server errors, and one with no reason phrase.
const STATUSES: [u16; 16] = [
    200, 200, 202, 204, 300, 301, 304, 400, 403, 404, 409, 413, 500, 502, 503, 599,
];

/// The response headers the parsers read, each a flag bit.
const HEADERS: [(&str, &str); 6] = [
    ("Content-Type", "application/x-amz-json-1.0"),
    ("Content-Type", "text/xml"),
    ("x-amzn-RequestId", "fuzz-request"),
    ("x-amzn-query-error", "Fuzz.Code;Sender"),
    ("x-amz-request-id", "fuzz-id"),
    ("Content-Type", "text/html"),
];

/// The CLI's parser defaults for `cli_timestamp_format=wire`: both values
/// as written.
const WIRE: ParseOptions<'static> = ParseOptions {
    timestamp: &wire_timestamp,
    blob: &wire_blob,
};

fn wire_timestamp(value: &Value) -> Result<Value, ParseError> {
    Ok(value.clone())
}

fn wire_blob(text: &str) -> Result<Value, ParseError> {
    Ok(Value::String(text.to_owned()))
}

/// A response for one of `models`' operations from any bytes, read by the
/// model's protocol under botocore's and the CLI's parser defaults: the
/// first byte picks the model and operation, the second the status, the
/// third the headers, and the rest is the body. A success parses or fails
/// with `ResponseParserError`; an error status gives a service error or
/// fails the same way.
pub fn responses(models: &[Model], bytes: &[u8]) {
    let [pick, status, flags, body @ ..] = bytes else {
        return;
    };
    let Some(model) = models.get(usize::from(pick & 3) % models.len().max(1)) else {
        return;
    };
    let (Some(protocol), operations) = (protocol::for_model(model), model.operations()) else {
        return;
    };
    let Some(operation) = operations.get(usize::from(pick >> 2) % operations.len().max(1)) else {
        return;
    };
    let headers = HEADERS
        .iter()
        .enumerate()
        .filter(|(bit, _)| flags & (1 << bit) != 0)
        .map(|(_, header)| *header);
    let response = HttpResponse {
        status: STATUSES[usize::from(status & 15)],
        headers: Headers::from_pairs(headers),
        body: body.to_vec(),
    };
    for options in [&ParseOptions::BOTOCORE, &WIRE] {
        if response.status < 300 {
            let _ = protocol.parse(model, operation, &response, options);
        } else {
            let _ = protocol.parse_error(model, operation, &response, options);
        }
    }
}

#[cfg(test)]
mod tests;
