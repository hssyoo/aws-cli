//! botocore's `ParamValidator`: every check, in its order, with the report
//! text `ValidationErrors.generate_report` writes and values printed with
//! Python's `str()`.
use crate::pyjson::{Options, dumps};
use crate::pyrepr::{py_str, py_type};
use crate::schema::{Bound, Flag, OperationSchema, Schema, ShapeType, TraitId, traits};
use crate::value::Value;

mod datetime;
mod number;
#[cfg(test)]
mod root;

/// The outcome of validating parameters.
#[derive(Clone, Debug, PartialEq)]
pub enum Validation {
    Valid,
    /// botocore would raise `ParamValidationError` with this report.
    Invalid(Report),
    /// botocore's answer depends on code this crate does not reproduce, so
    /// the Python route must decide.
    Undecided(Undecided),
}

/// What only the Python route can decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undecided {
    /// A timestamp value only `dateutil.parser.parse` can judge, or one for
    /// which `parse_timestamp` raises an exception the validator does not
    /// catch.
    Timestamp(String),
    /// A shape type botocore has no `_validate_<type>` for.
    UnknownType(Box<str>),
    /// A comparison of an integer beyond `i64` with a bound beyond 2^63.
    Comparison,
}

/// `ValidationErrors`: one formatted line per error, in report order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    errors: Vec<String>,
}

impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &str> {
        self.errors.iter().map(String::as_str)
    }

    /// `str(ParamValidationError(report=...))`.
    pub fn message(&self) -> String {
        format!("Parameter validation failed:\n{self}")
    }
}

impl std::fmt::Display for Report {
    /// `ValidationErrors.generate_report`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.errors.join("\n"))
    }
}

/// `ParamValidationDecorator.serialize_to_request`'s check: an operation
/// without an input shape takes any parameters.
pub fn validate_input(operation: OperationSchema<'_>, params: &Value) -> Validation {
    match operation.input() {
        Some(input) => validate(input, params),
        None => Validation::Valid,
    }
}

/// `ParamValidator().validate(params, shape)`.
pub fn validate(shape: Schema<'_>, params: &Value) -> Validation {
    let mut validator = Validator {
        report: Report::default(),
    };
    match validator.check(params, shape, "") {
        Err(undecided) => Validation::Undecided(undecided),
        Ok(()) if validator.report.errors.is_empty() => Validation::Valid,
        Ok(()) => Validation::Invalid(validator.report),
    }
}

type Checked = Result<(), Undecided>;

const DOCUMENT_TYPES: &str = "<class 'str'>, <class 'int'>, <class 'bool'>, <class 'float'>, \
    <class 'list'>, <class 'dict'>";

struct Validator {
    report: Report,
}

/// `ValidationErrors._get_name`.
fn display_name(name: &str) -> &str {
    if name.is_empty() {
        "input"
    } else {
        name.strip_prefix('.').unwrap_or(name)
    }
}

/// Whether botocore reads a structure as a `document` (or `union`): the
/// truthiness of the key on the merged reference, which the type says
/// unless a reference changed it (then it is a cold `c2j#` trait).
fn structure_is(shape: Schema<'_>, id: TraitId, expressed: ShapeType) -> bool {
    match shape.cold_trait(id.as_str()) {
        Some(value) => truthy(value),
        None => shape.shape_type() == expressed,
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: &serde_json::Value) -> bool {
    use serde_json::Value as Json;
    match value {
        Json::Null => false,
        Json::Bool(value) => *value,
        Json::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Json::String(text) => !text.is_empty(),
        Json::Array(items) => !items.is_empty(),
        Json::Object(map) => !map.is_empty(),
    }
}

/// A shape whose children are missing, which no front-end builds: Python
/// decides.
fn childless(shape: Schema<'_>) -> Undecided {
    Undecided::UnknownType(shape.type_name().into())
}

impl Validator {
    fn error(&mut self, line: String) {
        self.report.errors.push(line);
    }

    /// `ParamValidator._validate`: the jsonvalue-header and document checks
    /// replace the type's own.
    fn check(&mut self, params: &Value, shape: Schema<'_>, name: &str) -> Checked {
        let shape_type = shape.shape_type();
        if shape.flag(Flag::JsonValue)
            && shape.location() == Some("header")
            && shape_type == ShapeType::String
        {
            self.json_value(params, name);
            return Ok(());
        }
        let structure = matches!(
            shape_type,
            ShapeType::Structure | ShapeType::Document | ShapeType::Union
        );
        if structure && structure_is(shape, traits::DOCUMENT, ShapeType::Document) {
            self.document(params, name);
            return Ok(());
        }
        match shape_type {
            ShapeType::Structure | ShapeType::Document | ShapeType::Union => {
                self.structure(params, shape, name)
            }
            ShapeType::List => {
                let member = shape.list_member().ok_or_else(|| childless(shape))?;
                self.list(params, shape, member, name)
            }
            ShapeType::Map => {
                let key = shape.map_key().ok_or_else(|| childless(shape))?;
                let value = shape.map_value().ok_or_else(|| childless(shape))?;
                self.map(params, key, value, name)
            }
            ShapeType::String => self.string(params, shape, name),
            ShapeType::Integer | ShapeType::Long => self.integer(params, shape, name),
            ShapeType::Float | ShapeType::Double => self.double(params, shape, name),
            ShapeType::Boolean => {
                self.type_check(
                    params,
                    name,
                    matches!(params, Value::Bool(_)),
                    "<class 'bool'>",
                );
                Ok(())
            }
            ShapeType::Blob => {
                self.blob(params, name);
                Ok(())
            }
            ShapeType::Timestamp => self.timestamp(params, name),
            // botocore has no `_validate_<type>` for these.
            ShapeType::Byte
            | ShapeType::Short
            | ShapeType::BigInteger
            | ShapeType::BigDecimal
            | ShapeType::Unknown => Err(Undecided::UnknownType(shape.type_name().into())),
        }
    }

    /// `type_check`'s guard: reports and returns false for a wrong type.
    fn type_check(&mut self, params: &Value, name: &str, valid: bool, types: &str) -> bool {
        if !valid {
            self.invalid_type(params, name, types);
        }
        valid
    }

    fn invalid_type(&mut self, params: &Value, name: &str, types: &str) {
        self.error(format!(
            "Invalid type for parameter {}, value: {}, type: {}, valid types: {types}",
            display_name(name),
            py_str(params),
            py_type(params),
        ));
    }

    /// `_validate_jsonvalue_string`.
    fn json_value(&mut self, params: &Value, name: &str) {
        if let Err(error) = dumps(params, &Options::default(), None) {
            let name = display_name(name);
            self.error(format!(
                "Invalid parameter {name} must be json serializable: {error}"
            ));
        }
    }

    /// `_validate_document`: a dict's values are named by their key alone.
    fn document(&mut self, params: &Value, name: &str) {
        match params {
            Value::Null
            | Value::Bool(_)
            | Value::Integer(_)
            | Value::BigInteger(_)
            | Value::Float(_)
            | Value::String(_)
            | Value::PyString(_) => {}
            Value::Map(map) => {
                for (key, value) in map.iter() {
                    self.document(value, &key.text());
                }
            }
            Value::List(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.document(item, &format!("{name}[{index}]"));
                }
            }
            Value::Blob(_) | Value::Timestamp(_) => self.error(format!(
                "Invalid type for document parameter {}, value: {}, type: {}, valid types: \
                 {DOCUMENT_TYPES}",
                display_name(name),
                py_str(params),
                py_type(params),
            )),
        }
    }

    /// `_validate_structure`.
    fn structure(&mut self, params: &Value, shape: Schema<'_>, name: &str) -> Checked {
        let Value::Map(map) = params else {
            self.invalid_type(params, name, "<class 'dict'>");
            return Ok(());
        };
        let members = shape.members();
        let member_names = || {
            let names = members
                .clone()
                .map(|member| member.member_name().unwrap_or(""));
            names.collect::<Vec<_>>()
        };
        if structure_is(shape, traits::UNION, ShapeType::Union) && map.len() != 1 {
            let members = member_names().join(". ");
            let display = display_name(name);
            self.error(if map.is_empty() {
                // botocore's text has no space in "unionstructure".
                format!(
                    "Must set one of the following keys for tagged union\
                     structure {display}: {members}."
                )
            } else {
                format!(
                    "Invalid number of parameters set for tagged union structure {display}. \
                     Can only set one of the following keys: {members}."
                )
            });
        }
        for required in shape.required() {
            if !map.contains_key(required) {
                self.error(format!(
                    "Missing required parameter in {}: \"{required}\"",
                    display_name(name)
                ));
            }
        }
        let mut known = Vec::with_capacity(map.len());
        for (key, value) in map.iter() {
            let mut members = members.clone();
            match members.find(|member| *key == *member.member_name().unwrap_or("")) {
                Some(member) => known.push((key, value, member)),
                None => self.error(format!(
                    "Unknown parameter in {}: \"{}\", must be one of: {}",
                    display_name(name),
                    key.text(),
                    member_names().join(", ")
                )),
            }
        }
        for (key, value, member) in known {
            self.check(value, member, &format!("{name}.{}", key.text()))?;
        }
        Ok(())
    }

    /// `_validate_list`.
    fn list(
        &mut self,
        params: &Value,
        shape: Schema<'_>,
        member: Schema<'_>,
        name: &str,
    ) -> Checked {
        let Value::List(items) = params else {
            self.invalid_type(params, name, "<class 'list'>, <class 'tuple'>");
            return Ok(());
        };
        self.length(items.len(), shape, name);
        for (index, item) in items.iter().enumerate() {
            self.check(item, member, &format!("{name}[{index}]"))?;
        }
        Ok(())
    }

    /// `_validate_map`: each key against the key shape, then its value.
    fn map(&mut self, params: &Value, key: Schema<'_>, value: Schema<'_>, name: &str) -> Checked {
        let Value::Map(map) = params else {
            self.invalid_type(params, name, "<class 'dict'>");
            return Ok(());
        };
        for (entry_key, entry_value) in map.iter() {
            let key_value = entry_key.to_value();
            let shown = entry_key.text();
            self.check(&key_value, key, &format!("{name} (key: {shown})"))?;
            self.check(entry_value, value, &format!("{name}.{shown}"))?;
        }
        Ok(())
    }

    /// `_validate_string`: the length counts code points, as `len(str)` does.
    fn string(&mut self, params: &Value, shape: Schema<'_>, name: &str) -> Checked {
        if let Value::String(text) = params {
            self.length(text.chars().count(), shape, name);
        } else if let Value::PyString(text) = params {
            self.length(text.len(), shape, name);
        } else {
            self.invalid_type(params, name, "<class 'str'>");
        }
        Ok(())
    }

    /// `_validate_integer` and `_validate_long`: `bool` is an `int` in Python.
    fn integer(&mut self, params: &Value, shape: Schema<'_>, name: &str) -> Checked {
        let valid = matches!(
            params,
            Value::Integer(_) | Value::BigInteger(_) | Value::Bool(_)
        );
        if self.type_check(params, name, valid, "<class 'int'>") {
            self.range(params, shape, name)?;
        }
        Ok(())
    }

    /// `_validate_double` and `_validate_float`.
    fn double(&mut self, params: &Value, shape: Schema<'_>, name: &str) -> Checked {
        let valid = matches!(
            params,
            Value::Float(_) | Value::Integer(_) | Value::BigInteger(_) | Value::Bool(_)
        );
        let types = "<class 'float'>, <class 'decimal.Decimal'>, <class 'int'>";
        if self.type_check(params, name, valid, types) {
            self.range(params, shape, name)?;
        }
        Ok(())
    }

    /// `_validate_blob`: `bytes`, `bytearray` and `str` pass.
    fn blob(&mut self, params: &Value, name: &str) {
        if !matches!(
            params,
            Value::Blob(_) | Value::String(_) | Value::PyString(_)
        ) {
            let types = "<class 'bytes'>, <class 'bytearray'>, file-like object";
            self.invalid_type(params, name, types);
        }
    }

    /// `_validate_timestamp`.
    fn timestamp(&mut self, params: &Value, name: &str) -> Checked {
        if !datetime::accepted(params)? {
            let types = "<class 'datetime.datetime'>, timestamp-string";
            self.invalid_type(params, name, types);
        }
        Ok(())
    }

    /// `range_check` over a length, reported as `invalid length`.
    fn length(&mut self, length: usize, shape: Schema<'_>, name: &str) {
        let length = i64::try_from(length).unwrap_or(i64::MAX);
        if let Some(min) = Self::minimum(shape)
            && number::int_below(length, min)
        {
            let name = display_name(name);
            self.error(format!(
                "Invalid length for parameter {name}, value: {length}, valid min length: {min}"
            ));
        }
    }

    /// `range_check` over a number, reported as `invalid range`.
    fn range(&mut self, params: &Value, shape: Schema<'_>, name: &str) -> Checked {
        let Some(min) = Self::minimum(shape) else {
            return Ok(());
        };
        if number::value_below(params, min)? {
            self.error(format!(
                "Invalid value for parameter {}, value: {}, valid min value: {min}",
                display_name(name),
                py_str(params)
            ));
        }
        Ok(())
    }

    /// `min`, else the implicit minimum of 1 a host label has.
    fn minimum(shape: Schema<'_>) -> Option<Bound> {
        match shape.min() {
            Some(min) => Some(min),
            None if shape.flag(Flag::HostLabel) => Some(Bound::Integer(1)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests;
