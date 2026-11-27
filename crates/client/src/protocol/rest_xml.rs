//! The `rest-xml` protocol: botocore's `RestXMLSerializer` and
//! `RestXMLParser`, as an [`HttpBindingProtocol`] over the XML codec.
//!
//! What every REST protocol shares, `BaseRestSerializer` (the parameters
//! by `location`, the URI template, the query string, the headers) and
//! `BaseRestParser` (the status code, headers and request ids, the payload
//! or the body's members), is the `rest-json` protocol's; this module adds
//! the XML body both ways ([`crate::codec::xml`]), the XML scalar handlers
//! a header's text is read with, and the error envelope ([`parse`]).
//!
//! Built but not admitted: [`REST_XML`] is in [`super::SWITCHED_OFF`],
//! which only tests read, and not in [`super::IMPLEMENTED`], so admission
//! still refuses a rest-xml service.
use super::rest_json::{
    Payload, encode_payload, joined_query, no_items, partition, render_uri_template, split_uri,
};
use super::{BotocoreCompat, ClientProtocol, HttpBindingProtocol, ParseOptions, Serialized};
use super::{common, query::method};
use crate::codec::value::write_value;
use crate::codec::xml::{XML, XmlCodec, XmlSerializer};
use crate::error::{ParseError, SerializeError};
use crate::http::{Headers, HttpResponse};
use model::schema::{OperationSchema, Schema, TraitId, traits};
use model::serde::{Codec, FinishSerializer};
use model::shapes::Model;
use model::value::{Map, Value};

mod parse;

/// restXml, botocore's `rest-xml`. Its compat constants are the REST
/// parsers': the generic 5xx error; the error code is the envelope's.
pub static REST_XML: HttpBindingProtocol<XmlCodec> = HttpBindingProtocol {
    id: traits::REST_XML,
    codec: &XML,
    compat: BotocoreCompat::REST,
};

impl ClientProtocol for HttpBindingProtocol<XmlCodec> {
    fn protocol_id(&self) -> TraitId {
        self.id
    }
    fn payload_codec(&self) -> Option<&dyn Codec> {
        Some(self.codec)
    }
    fn compat(&self) -> &BotocoreCompat {
        &self.compat
    }

    /// `RestXMLSerializer.serialize_to_request`.
    fn serialize_request(
        &self,
        operation: OperationSchema<'_>,
        input: &Value,
        _model: &Model,
    ) -> Result<Serialized, SerializeError> {
        request(operation, input)
    }

    /// `BaseRestParser._do_parse` with `RestXMLParser`'s body.
    fn deserialize_response(
        &self,
        _operation: OperationSchema<'_>,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        parse::do_parse(shape, response, options).map(Value::Map)
    }

    /// `RestXMLParser._do_error_parse`.
    fn parse_error_metadata(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        parse::error_metadata(response)
    }

    /// `BaseRestParser._do_modeled_error_parse` with `RestXMLParser`'s
    /// body.
    fn deserialize_modeled_error(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        parse::do_modeled_error_parse(shape, response, options)
    }
}

/// `BaseRestSerializer.serialize_to_request` with `RestXMLSerializer`'s
/// body: `requestUri` with its labels filled in, the query string after
/// any query `requestUri` carries, the header members, then the payload or
/// the XML body. No `Content-Type` is set.
///
/// # Errors
/// An input the protocol cannot put on the wire.
fn request(operation: OperationSchema<'_>, input: &Value) -> Result<Serialized, SerializeError> {
    let method = method(operation)?;
    let host_prefix = common::host_prefix(operation, input)?;
    let template = || {
        operation
            .http_request_uri()
            .ok_or_else(|| SerializeError::new("'requestUri'"))
    };
    // `if shape is None`: the template as it is, labels and all.
    let Some(shape) = operation.input() else {
        let (path, query) = split_uri(template()?);
        return Ok(Serialized {
            method,
            path: path.to_owned(),
            query: query.unwrap_or_default().to_owned(),
            headers: Headers::new(),
            body: Vec::new(),
            host_prefix,
        });
    };
    let Value::Map(params) = input else {
        return Err(no_items(input));
    };
    let partitioned = partition(shape, params)?;
    let url_path = render_uri_template(template()?, &partitioned.uri)?;
    let (path, uri_query) = split_uri(&url_path);
    let query = joined_query(uri_query, &partitioned.query)?;
    let body = body(shape, Payload::of(shape)?, params, &partitioned.body)?;
    Ok(Serialized {
        method,
        path: path.to_owned(),
        query,
        headers: partitioned.headers,
        body,
        host_prefix,
    })
}

/// `BaseRestSerializer._serialize_payload` with `RestXMLSerializer`'s
/// rules: a blob or string payload as its bytes; another payload as XML
/// rooted at the member's `locationName`; nothing for an unset payload;
/// without a payload, the body members as XML rooted at the input's
/// `locationName`, or nothing when none is set.
fn body(
    shape: Schema<'_>,
    payload: Option<Payload<'_>>,
    params: &Map,
    body_members: &Map,
) -> Result<Vec<u8>, SerializeError> {
    if let Some(payload) = payload {
        let value = params
            .get(payload.name)
            .filter(|value| **value != Value::Null);
        return match (value, payload.raw) {
            (None, _) => Ok(Vec::new()),
            (Some(value), true) => encode_payload(value),
            (Some(value), false) => serialize_body_params(value, payload.shape),
        };
    }
    if body_members.is_empty() {
        return Ok(Vec::new());
    }
    serialize_body_params(&Value::Map(body_members.clone()), shape)
}

/// `RestXMLSerializer._serialize_body_params`: `value` as the element
/// `shape`'s `locationName` names, written as `ElementTree.tostring` does.
fn serialize_body_params(value: &Value, shape: Schema<'_>) -> Result<Vec<u8>, SerializeError> {
    let mut serializer = XmlSerializer::default();
    write_value(shape, value, &mut serializer)?;
    Ok(Box::new(serializer).finish()?)
}

#[cfg(test)]
mod tests;
