//! Codecs: the wire formats (JSON, XML, form) a protocol serializes and
//! parses through, each over [`model::serde`]'s traits, and the walks over
//! [`model::value::Value`] they share ([`value`]).

pub mod json;
pub mod value;
