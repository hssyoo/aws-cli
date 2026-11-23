//! `RestJSONParser`: [`rest`]'s `BaseRestParser` over a JSON body, read by
//! the `json` protocol's reader with `RestJSONParser`'s handlers, and the
//! rest-json error code.
use super::RestJson;
use crate::error::ParseError;
use crate::http::{Headers, HttpResponse};
use crate::protocol::ParseOptions;
use crate::protocol::common::{Family, short_type};
use crate::protocol::json::{Json, Reader, body_json, error_code};
use model::pyrepr::{float_from_text, int_from_text, py_repr};
use model::shapes::{Model, ShapeId, ShapeKind};
use model::value::{Map, Value};

mod rest;

impl Family for RestJson {
    /// `BaseRestParser._do_parse`.
    fn do_parse(
        &self,
        model: &Model,
        shape: Option<ShapeId>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        rest::do_parse(&reader(model, options), model, shape, response).map(Value::Map)
    }

    /// `RestJSONParser._do_error_parse`: `BaseJSONParser`'s message, code
    /// and request id, then the code `_inject_error_code` finds.
    fn do_error_parse(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        let body = initial_body_parse(&response.body)?;
        if !matches!(body, Value::Map(_)) {
            // `body.get('message', ...)` raises before any code is read.
            return Err(rest::no_attribute(&body, "get"));
        }
        let mut parsed = Json.do_error_parse(response)?;
        if let Some(code) = injected_code(&body, &response.headers)
            && let Some(Value::Map(error)) = parsed.get_mut("Error")
        {
            error.insert("Code".into(), code);
        }
        Ok(parsed)
    }

    /// `BaseRestParser._do_modeled_error_parse`.
    fn do_modeled_error_parse(
        &self,
        model: &Model,
        shape: ShapeId,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        rest::do_modeled_error_parse(&reader(model, options), model, shape, response)
    }
}

/// The `json` protocol's reader with [`handler`].
fn reader<'m, 'o>(model: &'m Model, options: &'o ParseOptions<'o>) -> Reader<'m, 'o> {
    Reader {
        model,
        options,
        handler: Some(handler),
    }
}

impl rest::Body for Reader<'_, '_> {
    type Parsed = Value;

    fn initial(&self, body: &[u8]) -> Result<Value, ParseError> {
        initial_body_parse(body)
    }

    fn parse(&self, shape: ShapeId, parsed: &Value) -> Result<Value, ParseError> {
        self.shape(shape, parsed)
    }

    fn value(&self, shape: ShapeId, value: &Value) -> Result<Value, ParseError> {
        self.shape(shape, value)
    }
}

/// `RestJSONParser._initial_body_parse`, `_parse_body_as_json`: the body
/// decoded first, so that one not UTF-8 fails with `UnicodeDecodeError`'s
/// text.
fn initial_body_parse(body: &[u8]) -> Result<Value, ParseError> {
    rest::decoded(body)?;
    body_json(body)
}

/// The handlers `RestJSONParser` and `BaseRestParser` override, which apply
/// to headers and body alike: `_handle_string` (a `jsonvalue` header),
/// `_handle_list` (a header's text split), and `_handle_boolean`,
/// `_handle_integer` and `_handle_float` with their `long` and `double`
/// aliases. `None` leaves the shape to `BaseJSONParser`.
fn handler(
    reader: &Reader<'_, '_>,
    shape: ShapeId,
    value: &Value,
) -> Option<Result<Value, ParseError>> {
    let resolved = reader.model.shape(shape);
    match &resolved.kind {
        ShapeKind::String => rest::json_value(resolved, value),
        ShapeKind::List { member } => rest::header_items(resolved, value)
            .map(|items| items.and_then(|items| reader.list(*member, &items))),
        ShapeKind::Boolean => Some(Ok(Value::Bool(ensure_boolean(value)))),
        ShapeKind::Integer | ShapeKind::Long => Some(int(value)),
        ShapeKind::Float | ShapeKind::Double => Some(float(value)),
        _ => None,
    }
}

/// `ensure_boolean`: a bool as it is, a string by whether it is `true` in
/// any case, anything else false.
fn ensure_boolean(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text.to_lowercase() == "true",
        _ => false,
    }
}

/// `int(value)`: a float truncated, a bool as 0 or 1, text as a base-10
/// literal.
fn int(value: &Value) -> Result<Value, ParseError> {
    match value {
        Value::Integer(_) | Value::BigInteger(_) => Ok(value.clone()),
        Value::Bool(flag) => Ok(Value::Integer(i64::from(*flag))),
        Value::Float(number) => truncated(*number),
        Value::String(text) => int_from_text(text).map_err(ParseError::new),
        other => Err(ParseError::new(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            short_type(other)
        ))),
    }
}

/// `int(number)` of a float: toward zero, of any size.
fn truncated(number: f64) -> Result<Value, ParseError> {
    if number.is_nan() {
        return Err(ParseError::new("cannot convert float NaN to integer"));
    }
    if number.is_infinite() {
        return Err(ParseError::new("cannot convert float infinity to integer"));
    }
    let whole = number.trunc();
    // -2**63 and 2**63, which are exact as floats.
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&whole) {
        return Ok(Value::Integer(whole as i64));
    }
    Ok(Value::BigInteger(whole_digits(whole).into()))
}

/// The decimal digits of a whole float beyond `i64`: its 53-bit mantissa
/// doubled once per binary exponent, in base 10**9.
fn whole_digits(whole: f64) -> String {
    const BASE: u64 = 1_000_000_000;
    let bits = whole.to_bits();
    let exponent = ((bits >> 52) & 0x7ff).saturating_sub(1075);
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    let mut limbs = vec![
        mantissa % BASE,
        mantissa / BASE % BASE,
        mantissa / BASE / BASE,
    ];
    for _ in 0..exponent {
        let mut carry = 0;
        for limb in &mut limbs {
            let doubled = *limb * 2 + carry;
            (*limb, carry) = (doubled % BASE, doubled / BASE);
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut digits = String::from(if whole < 0.0 { "-" } else { "" });
    for (index, limb) in limbs.iter().rev().enumerate() {
        digits.push_str(&if index == 0 {
            limb.to_string()
        } else {
            format!("{limb:09}")
        });
    }
    digits
}

/// `float(value)`: an int as the nearest float, a bool as 0.0 or 1.0, text
/// as a literal (`NaN` and `Infinity` among them).
fn float(value: &Value) -> Result<Value, ParseError> {
    let number = match value {
        Value::Float(number) => *number,
        Value::Integer(number) => *number as f64,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::BigInteger(digits) => digits
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or_else(|| ParseError::new("int too large to convert to float"))?,
        Value::String(text) => float_from_text(text).ok_or_else(|| {
            ParseError::new(format!(
                "could not convert string to float: {}",
                py_repr(value)
            ))
        })?,
        other => {
            return Err(ParseError::new(format!(
                "float() argument must be a string or a real number, not '{}'",
                short_type(other)
            )));
        }
    };
    Ok(Value::Float(number))
}

/// `RestJSONParser._inject_error_code`'s code: `x-amzn-errortype`, else the
/// body's `code`, else its `Code`, a text one without what follows a `:` and
/// what precedes the last `#`. `None`, which leaves `BaseJSONParser`'s code,
/// when there is none or it is `null`.
fn injected_code(body: &Value, headers: &Headers) -> Option<Value> {
    let code = match rest::header(headers, "x-amzn-errortype") {
        Some(code) => Value::String(code),
        None => match body {
            Value::Map(body) => body.get("code").or_else(|| body.get("Code"))?.clone(),
            _ => return None,
        },
    };
    match code {
        Value::Null => None,
        Value::String(text) => Some(Value::String(error_code(&text))),
        other => Some(other),
    }
}

#[cfg(test)]
mod tests;
