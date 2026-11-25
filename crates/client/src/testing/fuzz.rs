//! The bodies of the fuzz targets over this crate's readers
//! (`fuzz/fuzz_targets/client_xml.rs`, `protocol_responses.rs`,
//! `paginator_token.rs`), which the stable replay test (its `tests`) also
//! runs. No input may panic.
use crate::error::ParseError;
use crate::http::{Headers, HttpResponse};
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

/// The response headers the parsers read, each a flag bit: rest-json's
/// error type names a modelled error of the fake rest-json model, with the
/// suffix `_inject_error_code` drops.
const HEADERS: [(&str, &str); 7] = [
    ("Content-Type", "application/x-amz-json-1.0"),
    ("Content-Type", "text/xml"),
    ("x-amzn-RequestId", "fuzz-request"),
    ("x-amzn-query-error", "Fuzz.Code;Sender"),
    ("x-amz-request-id", "fuzz-id"),
    ("Content-Type", "text/html"),
    ("x-amzn-errortype", "ItemLocked:http://fuzz.example/"),
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

/// The input tokens a paginator may name: none, botocore's usual one, or
/// two, as a paginator with a compound token has.
const INPUT_TOKENS: [&[&str]; 3] = [&[], &["NextToken"], &["Marker", "NextRecord"]];

/// `PageIterator._parse_starting_token` of any text: the first byte picks
/// the paginator's input tokens and the rest, as UTF-8 with any invalid
/// bytes replaced, is the token. It reads the same twice, and a token that
/// decodes to exactly the input tokens encodes (the `resume_token` setter)
/// to one that decodes to the same token and offset.
pub fn starting_token(bytes: &[u8]) {
    use crate::paginate::{TRUNCATE_AMOUNT, parse_starting_token, resume_token};
    let [pick, raw @ ..] = bytes else {
        return;
    };
    let names: Vec<String> = INPUT_TOKENS[usize::from(*pick) % INPUT_TOKENS.len()]
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let raw = String::from_utf8_lossy(raw);
    let first = parse_starting_token(&raw, &names);
    assert_eq!(first, parse_starting_token(&raw, &names));
    let Ok(parsed) = first else {
        return;
    };
    let mut keys: Vec<&model::value::Key> = parsed.token.keys().collect();
    keys.sort_unstable();
    let mut expected: Vec<model::value::Key> = names
        .iter()
        .map(|name| model::value::Key::from(name.as_str()))
        .collect();
    expected.sort_unstable();
    if !keys.iter().copied().eq(expected.iter()) {
        return;
    }
    let mut value = parsed.token.clone();
    value.insert(TRUNCATE_AMOUNT.to_owned(), parsed.index.clone());
    let Ok(encoded) = resume_token(&value, &names) else {
        return;
    };
    assert_eq!(parse_starting_token(&encoded, &names), Ok(parsed));
}
