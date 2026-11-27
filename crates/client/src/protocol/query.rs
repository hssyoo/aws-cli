//! The `query` protocol: botocore's `QuerySerializer` (a form-encoded POST
//! body, [`crate::codec::form`]) and `QueryParser` (an XML response,
//! [`crate::codec::xml`]).
use super::common::{self, Family};
use super::{ParseOptions, Protocol, Serialized};
use crate::codec::form::{FORM, FormSerializer};
use crate::codec::value::{build_value, write_value};
use crate::codec::xml::{self, Node, XmlDeserializer, by_name, lookup, replaced};
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse, Method};
use crate::xml::Element;
use model::pyrepr::py_str;
use model::schema::{OperationSchema, Schema};
use model::serde::{Codec, FinishSerializer};
use model::shapes::Model;
use model::value::{Map, Value};

/// The `query` protocol.
#[derive(Debug)]
pub struct Query;

impl Protocol for Query {
    fn name(&self) -> &'static str {
        "query"
    }
    fn epoch_timestamps(&self) -> bool {
        false
    }

    /// `QuerySerializer.serialize_to_request`.
    fn serialize(
        &self,
        model: &Model,
        operation: OperationSchema<'_>,
        input: &Value,
    ) -> Result<Serialized, SerializeError> {
        let version = model
            .schema()
            .service()
            .version()
            .ok_or_else(|| SerializeError::new("'apiVersion'"))?;
        let mut form = FormSerializer::default();
        form.set("Action", operation.name().as_bytes().to_vec());
        form.set("Version", version.as_bytes().to_vec());
        if let Some(shape) = operation.input() {
            write_value(shape, input, &mut form)?;
        }
        let mut headers = Headers::new();
        headers.set("Content-Type", FORM.media_type());
        Ok(Serialized {
            method: method(operation)?,
            path: "/".into(),
            query: String::new(),
            headers,
            body: Box::new(form).finish()?,
            host_prefix: common::host_prefix(operation, input)?,
        })
    }

    fn parse(
        &self,
        _model: &Model,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        common::parse(self, operation.output(), response, options)
    }

    fn parse_error(
        &self,
        model: &Model,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<ServiceError, ParseError> {
        common::parse_error(self, model, operation, response, options)
    }
}

/// `http.method`, `POST` by default.
pub(super) fn method(operation: OperationSchema<'_>) -> Result<Method, SerializeError> {
    let name = operation.http_method().unwrap_or("POST");
    Method::parse(name).ok_or_else(|| SerializeError::new(format!("unsupported method {name}")))
}

impl Family for Query {
    /// `_parse_body_as_xml` with the metadata injected.
    fn do_parse(
        &self,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        let root = xml::parse(&response.body)?;
        let mut parsed = match shape {
            Some(shape) => body_shape(shape, &root, options)?,
            None => Map::new(),
        };
        if let Some(metadata) = lookup(&by_name(&root), "ResponseMetadata") {
            let fields = by_name(metadata.first())
                .into_iter()
                .map(|(name, node)| {
                    (
                        name,
                        node.first().text.clone().map_or(Value::Null, Value::String),
                    )
                })
                .collect();
            parsed.insert("ResponseMetadata".into(), Value::Map(fields));
        }
        Ok(Value::Map(parsed))
    }

    /// `QueryParser._do_error_parse`.
    fn do_error_parse(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        let root = xml::parse(&response.body)?;
        let mut parsed = replaced(by_name(&root));
        if let Some(errors) = parsed.remove("Errors") {
            let Value::Map(errors) = errors else {
                return Err(ParseError::new(
                    "dictionary update sequence element #0 has length 1; 2 is required",
                ));
            };
            for (key, value) in errors {
                parsed.insert_key(key, value);
            }
        }
        if let Some(id) = parsed.remove("RequestId") {
            let mut metadata = Map::new();
            metadata.insert("RequestId".into(), id);
            parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
        }
        Ok(parsed)
    }

    /// `_parse_body_as_xml` without the metadata.
    fn do_modeled_error_parse(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        body_shape(shape, &xml::parse(&response.body)?, options)
    }
}

/// The shape parsed from the root, or from the `resultWrapper` element.
fn body_shape(
    shape: Schema<'_>,
    root: &Element,
    options: &ParseOptions<'_>,
) -> Result<Map, ParseError> {
    let start = match shape.result_wrapper() {
        Some(wrapper) => lookup(&by_name(root), wrapper)
            .cloned()
            .ok_or_else(|| ParseError::new(format!("'{wrapper}'")))?,
        None => Node::One(root),
    };
    match build_value(shape, &mut XmlDeserializer::new(start), options)? {
        Value::Map(map) => Ok(map),
        other => Err(ParseError::new(format!(
            "expected a structure, parsed {}",
            py_str(&other)
        ))),
    }
}

#[cfg(test)]
mod tests;
