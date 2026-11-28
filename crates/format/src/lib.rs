//! A response printed as the Python CLI prints it: `--output json`, `text`
//! and `off` (`awscli.formatter`, `awscli.text`), through the pager as
//! `OutputStreamFactory` sends it, with the CLI's timestamp handling. A
//! paginated response's text is written page by page ([`pages`]); its JSON
//! is the full result's, which the paginator builds.
//!
//! `--query` is applied as `Formatter` applies it ([`Query`]): to the
//! response after its `ResponseMetadata` is dropped, to the full result of a
//! paginated one, and by `TextFormatter` to each page it writes.
//!
//! `table`, `yaml` and `yaml-stream` stay on the Python route
//! ([`OutputFormat::from_name`] knows only the three here).
#![forbid(unsafe_code)]
use model::value::Value;

pub mod json;
pub mod pager;
pub mod pages;
pub mod stream;
pub mod text;
pub mod timestamps;

pub use pager::{Chain, Destination, NeedsPython, Outcome, Profile, SpawnError};
pub use text::PythonError;
pub use timestamps::{TimestampError, TimestampFormat, TimestampParser};

/// The `--output` formats printed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Text,
    Off,
}

impl OutputFormat {
    /// The format `name` selects in `CLI_OUTPUT_FORMATS`, if printed here.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "json" => Some(Self::Json),
            "text" => Some(Self::Text),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// Whether the Python formatter swallows an `OSError` from writing
    /// (`FullyBufferedFormatter.__call__`); `TextFormatter` lets it reach
    /// the CLI's handlers, unless the pager's stream swallows it.
    pub const fn swallows_write_errors(self) -> bool {
        matches!(self, Self::Json | Self::Off)
    }
}

/// `--query`, compiled: what `ParsedResult.search` gives for a value.
pub trait Query {
    /// The expression's result for `value`.
    ///
    /// # Errors
    /// The exception the search raises, as `str()` gives it.
    fn search(&self, value: &Value) -> Result<Value, PythonError>;
}

/// `Formatter._apply_query_if_needed`: `value` searched with `query`, if
/// there is one.
///
/// # Errors
/// The exception the search raised.
pub fn apply_query(value: Value, query: Option<&dyn Query>) -> Result<Value, PythonError> {
    match query {
        Some(query) => query.search(&value),
        None => Ok(value),
    }
}

/// Output the formatter wrote before raising.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partial {
    pub written: Vec<u8>,
    /// A write was made, even the one that failed, which starts the pager.
    pub started: bool,
    pub error: PythonError,
}

/// What the formatter for `format` writes for a non-paginated `response`,
/// after `Formatter._remove_request_id` drops its `ResponseMetadata`, to a
/// stream encoding with `errors` ([`Destination::errors`]).
///
/// # Errors
/// What the formatter raised, after the output it wrote before.
pub fn render(
    response: Value,
    format: OutputFormat,
    errors: pytext::codec::Errors,
) -> Result<Vec<u8>, Partial> {
    render_with(response, format, errors, None)
}

/// [`render`], with the response then searched with `query`
/// (`_get_transformed_response_for_output`), as every formatter but
/// `OffFormatter` searches it. The search writes nothing, so its exception
/// starts no pager.
///
/// # Errors
/// What the search or the formatter raised, after the output the formatter
/// wrote before.
pub fn render_with(
    mut response: Value,
    format: OutputFormat,
    errors: pytext::codec::Errors,
    query: Option<&dyn Query>,
) -> Result<Vec<u8>, Partial> {
    if format != OutputFormat::Off
        && let Some(class) = not_iterable(&response)
    {
        // `'ResponseMetadata' in response_data` raises for these, with
        // CPython 3.14's text.
        return Err(Partial {
            written: Vec::new(),
            started: false,
            error: PythonError {
                message: format!("argument of type '{class}' is not a container or iterable"),
            },
        });
    }
    // `_remove_request_id` pops it from the dict, which no one reads after.
    if let Value::Map(map) = &mut response {
        map.remove("ResponseMetadata");
    }
    if format != OutputFormat::Off {
        response = apply_query(response, query).map_err(|error| Partial {
            written: Vec::new(),
            started: false,
            error,
        })?;
    }
    let response = &response;
    let mut out = stream::Encoded::new(errors);
    let formatted = match format {
        OutputFormat::Json => json::json(response, &mut out),
        OutputFormat::Off => Ok(()),
        OutputFormat::Text => text::format_text(response, &mut out),
    };
    match formatted {
        Ok(()) => Ok(out.into_bytes()),
        Err(error) => Err(Partial {
            started: out.wrote(),
            written: out.into_bytes(),
            error,
        }),
    }
}

/// `type(value).__name__` of a value `in` cannot search.
fn not_iterable(value: &Value) -> Option<&'static str> {
    match value {
        Value::Null => Some("NoneType"),
        Value::Bool(_) => Some("bool"),
        Value::Integer(_) | Value::BigInteger(_) => Some("int"),
        Value::Float(_) => Some("float"),
        Value::Timestamp(_) => Some("datetime"),
        Value::String(_) | Value::PyString(_) | Value::Blob(_) | Value::List(_) | Value::Map(_) => {
            None
        }
    }
}

/// `text` written to stdout as CPython's `sys.stdout` takes it: buffered,
/// and line-buffered on a terminal, which flushes each line as it is
/// written, so a failure to flush there is the write's.
///
/// # Errors
/// The write failed.
pub fn write_stdout(
    stdout: &mut dyn std::io::Write,
    text: &[u8],
    line_buffered: bool,
) -> std::io::Result<()> {
    stdout.write_all(text)?;
    if line_buffered {
        stdout.flush()?;
    }
    Ok(())
}

/// Where delivered output went.
#[derive(Debug)]
pub enum Delivered {
    /// Written to stdout; the write's error, if any, for the caller to
    /// treat as [`OutputFormat::swallows_write_errors`] says.
    Stdout(std::io::Result<()>),
    /// Through the pager, or nothing to page, so no pager started.
    Paged(Outcome),
}

/// Writes `output` where `destination` says: to `stdout` ([`write_stdout`],
/// line-buffered on a terminal), or to a pager started with `env`
/// ([`pager::environment`]) only when there is output. Stdout is then
/// flushed as the formatter's `_flush_stream` flushes it, which passes over
/// an `OSError`: what that leaves unwritten, the exit-time flush reports.
///
/// # Errors
/// The pager could not start.
pub fn deliver(
    destination: &Destination,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
    output: &[u8],
    stdout: &mut dyn std::io::Write,
    line_buffered: bool,
    interrupt: &cli_core::interrupt::State,
) -> Result<Delivered, SpawnError> {
    match destination {
        Destination::Stdout => {
            let written = write_stdout(stdout, output, line_buffered);
            let _ = stdout.flush();
            Ok(Delivered::Stdout(written))
        }
        Destination::Pager(_) if output.is_empty() => Ok(Delivered::Paged(Outcome::Exited)),
        Destination::Pager(argv) => pager::run(argv, env, output, interrupt).map(Delivered::Paged),
    }
}

#[cfg(test)]
mod tests;
