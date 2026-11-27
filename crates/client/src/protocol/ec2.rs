//! The `ec2` protocol, ec2Query: botocore's `EC2Serializer` (the query
//! form under [`crate::codec::form::EC2_FORM`]'s names and lists) and
//! `EC2QueryParser` (the query parser with the request id in `requestId`,
//! errors in `<Response><Errors><Error>` and `RequestID`).
//!
//! Built but not admitted: it is registered in [`super::SWITCHED_OFF`]
//! only, so admission still refuses `ec2` and only tests reach it.
use super::query::{self, body_shape};
use super::{AWS_QUERY, BotocoreCompat, ClientProtocol, ParseOptions, Serialized, common};
use crate::codec::form::{Dialect, EC2_FORM, FormSerializer};
use crate::codec::value::write_value;
use crate::codec::xml::{self, ErrorRoot, by_name, lookup};
use crate::error::{ParseError, SerializeError};
use crate::http::{Headers, HttpResponse};
use model::schema::{OperationSchema, Schema, TraitId, traits};
use model::serde::{Codec, FinishSerializer};
use model::shapes::Model;
use model::value::{Map, Value};

/// ec2Query: a form-encoded request and an XML response, as awsQuery, with
/// `EC2Serializer`'s and `EC2QueryParser`'s departures from it.
#[derive(Debug)]
pub struct Ec2QueryProtocol {
    compat: BotocoreCompat,
}

/// ec2Query, botocore's `ec2`.
pub static EC2_QUERY: Ec2QueryProtocol = Ec2QueryProtocol {
    compat: BotocoreCompat::RPC,
};

impl ClientProtocol for Ec2QueryProtocol {
    fn protocol_id(&self) -> TraitId {
        traits::EC2_QUERY
    }
    fn payload_codec(&self) -> Option<&dyn Codec> {
        None
    }
    fn compat(&self) -> &BotocoreCompat {
        &self.compat
    }

    /// `QuerySerializer.serialize_to_request` with `EC2Serializer`'s names.
    fn serialize_request(
        &self,
        operation: OperationSchema<'_>,
        input: &Value,
        model: &Model,
    ) -> Result<Serialized, SerializeError> {
        let version = model
            .schema()
            .service()
            .version()
            .ok_or_else(|| SerializeError::new("'apiVersion'"))?;
        let mut form = FormSerializer::new(Dialect::Ec2);
        form.set("Action", operation.name().as_bytes().to_vec());
        form.set("Version", version.as_bytes().to_vec());
        if let Some(shape) = operation.input() {
            write_value(shape, input, &mut form)?;
        }
        let mut headers = Headers::new();
        headers.set("Content-Type", EC2_FORM.media_type());
        Ok(Serialized {
            method: query::method(operation)?,
            path: "/".into(),
            query: String::new(),
            headers,
            body: Box::new(form).finish()?,
            host_prefix: common::host_prefix(operation, input)?,
        })
    }

    /// `_parse_body_as_xml` with `EC2QueryParser._inject_response_metadata`:
    /// a root `requestId` element's text as the `RequestId`.
    fn deserialize_response(
        &self,
        _operation: OperationSchema<'_>,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        let root = xml::parse(&response.body)?;
        let mut parsed = match shape {
            Some(shape) => body_shape(shape, &root, ErrorRoot::Errors, options)?,
            None => Map::new(),
        };
        if let Some(node) = lookup(&by_name(&root), "requestId") {
            let id = node.first().text.clone().map_or(Value::Null, Value::String);
            let mut metadata = Map::new();
            metadata.insert("RequestId".into(), id);
            parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
        }
        Ok(Value::Map(parsed))
    }

    /// `EC2QueryParser._do_error_parse`: the query parser's, then a root
    /// `RequestID` (not `RequestId`) moved into `ResponseMetadata`.
    fn parse_error_metadata(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        let mut parsed = AWS_QUERY.parse_error_metadata(response)?;
        if let Some(id) = parsed.remove("RequestID") {
            let mut metadata = Map::new();
            metadata.insert("RequestId".into(), id);
            parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
        }
        Ok(parsed)
    }

    /// `_parse_body_as_xml` without the metadata, an error shape read from
    /// `EC2QueryParser._get_error_root`.
    fn deserialize_modeled_error(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        let root = xml::parse(&response.body)?;
        body_shape(shape, &root, ErrorRoot::Errors, options)
    }
}

#[cfg(test)]
mod tests;
