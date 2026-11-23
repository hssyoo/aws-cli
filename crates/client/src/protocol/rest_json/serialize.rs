//! `RestJSONSerializer`: the operation's `requestUri` with its labels, the
//! query string, the headers, and the payload or JSON body.
use crate::error::SerializeError;
use crate::protocol::Serialized;
use model::shapes::{Model, Operation};
use model::value::Value;

/// `RestJSONSerializer.serialize_to_request`.
///
/// # Errors
/// An input the protocol cannot put on the wire.
pub(super) fn request(
    _model: &Model,
    _operation: &Operation,
    _input: &Value,
) -> Result<Serialized, SerializeError> {
    Err(SerializeError::new(
        "rest-json requests are not serialized yet",
    ))
}
