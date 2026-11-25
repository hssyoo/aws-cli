//! botocore's `ParamValidator`: every check, in its order, with the report
//! text `ValidationErrors.generate_report` writes and values printed with
//! Python's `str()`.
use crate::pyjson::{Options, dumps};
use crate::pyrepr::{py_str, py_type};
use crate::shapes::{Bound, Flag, Model, Operation, ShapeId, ShapeKind};
use crate::value::Value;

mod datetime;
mod number;

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
pub fn validate_input(model: &Model, operation: &Operation, params: &Value) -> Validation {
    match operation.input {
        Some(input) => validate(model, input, params),
        None => Validation::Valid,
    }
}

/// `ParamValidator().validate(params, shape)`.
pub fn validate(model: &Model, shape: ShapeId, params: &Value) -> Validation {
    let mut validator = Validator {
        model,
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

struct Validator<'a> {
    model: &'a Model,
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

impl Validator<'_> {
    fn error(&mut self, line: String) {
        self.report.errors.push(line);
    }

    /// `ParamValidator._validate`: the jsonvalue-header and document checks
    /// replace the type's own.
    fn check(&mut self, params: &Value, shape: ShapeId, name: &str) -> Checked {
        let resolved = self.model.shape(shape);
        let traits = &resolved.traits;
        if traits.is(Flag::JsonValue)
            && traits.location.as_deref() == Some("header")
            && resolved.kind == ShapeKind::String
        {
            self.json_value(params, name);
            return Ok(());
        }
        if traits.is(Flag::Document) && matches!(resolved.kind, ShapeKind::Structure { .. }) {
            self.document(params, name);
            return Ok(());
        }
        match &resolved.kind {
            ShapeKind::Structure { .. } => self.structure(params, shape, name),
            ShapeKind::List { member } => self.list(params, shape, *member, name),
            ShapeKind::Map { key, value } => self.map(params, *key, *value, name),
            ShapeKind::String => self.string(params, shape, name),
            ShapeKind::Integer | ShapeKind::Long => self.integer(params, shape, name),
            ShapeKind::Float | ShapeKind::Double => self.double(params, shape, name),
            ShapeKind::Boolean => {
                self.type_check(
                    params,
                    name,
                    matches!(params, Value::Bool(_)),
                    "<class 'bool'>",
                );
                Ok(())
            }
            ShapeKind::Blob => {
                self.blob(params, name);
                Ok(())
            }
            ShapeKind::Timestamp => self.timestamp(params, name),
            ShapeKind::Other(type_name) => Err(Undecided::UnknownType(type_name.clone())),
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
    fn structure(&mut self, params: &Value, shape: ShapeId, name: &str) -> Checked {
        let Value::Map(map) = params else {
            self.invalid_type(params, name, "<class 'dict'>");
            return Ok(());
        };
        let model = self.model;
        let traits = &model.shape(shape).traits;
        let members = model.members(shape);
        let member_names = || members.iter().map(|m| &*m.name).collect::<Vec<_>>();
        if traits.is(Flag::Union) && map.len() != 1 {
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
        for required in traits.required.iter() {
            if !map.contains_key(required) {
                self.error(format!(
                    "Missing required parameter in {}: \"{required}\"",
                    display_name(name)
                ));
            }
        }
        let mut known = Vec::with_capacity(map.len());
        for (key, value) in map.iter() {
            match members.iter().find(|member| *key == *member.name) {
                Some(member) => known.push((key, value, member.shape)),
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
    fn list(&mut self, params: &Value, shape: ShapeId, member: ShapeId, name: &str) -> Checked {
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
    fn map(&mut self, params: &Value, key: ShapeId, value: ShapeId, name: &str) -> Checked {
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
    fn string(&mut self, params: &Value, shape: ShapeId, name: &str) -> Checked {
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
    fn integer(&mut self, params: &Value, shape: ShapeId, name: &str) -> Checked {
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
    fn double(&mut self, params: &Value, shape: ShapeId, name: &str) -> Checked {
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
    fn length(&mut self, length: usize, shape: ShapeId, name: &str) {
        let length = i64::try_from(length).unwrap_or(i64::MAX);
        if let Some(min) = self.minimum(shape)
            && number::int_below(length, min)
        {
            let name = display_name(name);
            self.error(format!(
                "Invalid length for parameter {name}, value: {length}, valid min length: {min}"
            ));
        }
    }

    /// `range_check` over a number, reported as `invalid range`.
    fn range(&mut self, params: &Value, shape: ShapeId, name: &str) -> Checked {
        let Some(min) = self.minimum(shape) else {
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
    fn minimum(&self, shape: ShapeId) -> Option<Bound> {
        let traits = &self.model.shape(shape).traits;
        match traits.min {
            Some(min) => Some(min),
            None if traits.is(Flag::HostLabel) => Some(Bound::Integer(1)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests;
