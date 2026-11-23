//! A response printed as the Python CLI prints it: `--output json`, `text`
//! and `off` (`awscli.formatter`, `awscli.text`), through the pager as
//! `OutputStreamFactory` sends it, with the CLI's timestamp handling.
//!
//! `table`, `yaml` and `yaml-stream` stay on the Python route
//! ([`OutputFormat::from_name`] knows only the three here).
#![forbid(unsafe_code)]
use model::value::Value;

pub mod json;
pub mod pager;
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

/// Output the formatter wrote before raising.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partial {
    pub written: String,
    pub error: PythonError,
}

/// What the formatter for `format` writes for a non-paginated `response`,
/// after `Formatter._remove_request_id` drops its `ResponseMetadata`.
pub fn render(response: &Value, format: OutputFormat) -> Result<String, Partial> {
    let trimmed;
    let response = match response {
        Value::Map(map) if map.contains_key("ResponseMetadata") => {
            let mut map = map.clone();
            map.remove("ResponseMetadata");
            trimmed = Value::Map(map);
            &trimmed
        }
        other => other,
    };
    match format {
        OutputFormat::Json => Ok(json::json(response)),
        OutputFormat::Off => Ok(String::new()),
        OutputFormat::Text => {
            let mut written = String::new();
            match text::format_text(response, &mut written) {
                Ok(()) => Ok(written),
                Err(error) => Err(Partial { written, error }),
            }
        }
    }
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

/// Writes `output` where `destination` says: to `stdout`, or to a pager
/// started with `env` ([`pager::environment`]) only when there is output.
///
/// # Errors
/// The pager could not start.
pub fn deliver(
    destination: &Destination,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
    output: &str,
    stdout: &mut dyn std::io::Write,
    interrupt: &cli_core::interrupt::State,
) -> Result<Delivered, SpawnError> {
    match destination {
        Destination::Stdout => Ok(Delivered::Stdout(
            stdout
                .write_all(output.as_bytes())
                .and_then(|()| stdout.flush()),
        )),
        Destination::Pager(_) if output.is_empty() => Ok(Delivered::Paged(Outcome::Exited)),
        Destination::Pager(argv) => {
            pager::run(argv, env, output.as_bytes(), interrupt).map(Delivered::Paged)
        }
    }
}

#[cfg(test)]
mod tests;
