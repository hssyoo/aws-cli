//! The request a completed parse binds, as `ServiceOperation.
//! _build_call_parameters` builds it: one argument at a time in the table's
//! order, each supplied value under its member's name, as `add_to_params`
//! stores it (`CLIArgument`, `BooleanArgument`). An unset value adds
//! nothing. The client keeps this order on the wire.
//!
//! The unpack loop already ran (`awscli_argparse::complete`): every value
//! here is a string, an integer or a boolean member's, as the derived table
//! says (`command::models::Unpack`), because any other supplied argument
//! stopped it at its destination.
use awscli_argparse::namespace::{Namespace, Value as Parsed};
use cli_core::cli_error::PythonReason;
use command::models::{Origin, Table, Unpack};
use model::value::{Map, Value};

/// The operation's input from the unpacked `values`, or why Python binds
/// it.
pub fn bind(table: &Table, values: &Namespace) -> Result<Value, PythonReason> {
    let mut input = Map::new();
    for argument in &table.arguments {
        let dest = argument
            .dest
            .as_deref()
            .map_or_else(|| argument.name.replace('-', "_"), str::to_owned);
        let Some(value) = values.value(&dest).filter(|value| !value.is_null()) else {
            continue;
        };
        let (Origin::Member { member, .. }, false) =
            (&argument.origin, argument.unpack == Unpack::Deferred)
        else {
            return Err(PythonReason::UnsupportedOption);
        };
        input.insert(member.to_string(), scalar(value)?);
    }
    Ok(Value::Map(input))
}

/// A string, an integer (any magnitude) or a boolean as the request holds
/// it.
fn scalar(value: &Parsed) -> Result<Value, PythonReason> {
    match value {
        Parsed::String(text) => Ok(Value::String(text.clone())),
        Parsed::Bool(flag) => Ok(Value::Bool(*flag)),
        Parsed::Number(digits) => Ok(digits.parse().map_or_else(
            |_| Value::BigInteger(digits.as_str().into()),
            Value::Integer,
        )),
        _ => Err(PythonReason::UnsupportedValue),
    }
}

#[cfg(test)]
mod tests;
