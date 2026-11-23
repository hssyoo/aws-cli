//! How a formatted command's response is printed, resolved once by the
//! router's settings stage and carried to the invocation, which reads it
//! and judges nothing again.
use awscli_config::ParsedFiles;
use cli_core::cli_error::PythonReason;
use cli_core::{EnvMap, aws_profile, shared_config};
/// The formats a native formatter writes, which a router names without
/// depending on `format`.
pub use format::OutputFormat;
use format::{TimestampFormat, TimestampParser};
use std::ffi::OsString;

/// The resolved output settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSettings {
    /// `--output`, else `AWS_DEFAULT_OUTPUT`, else the profile's `output`,
    /// else `json` (`CLIDriver._construct_cli_output_chain`).
    pub format: OutputFormat,
    /// `add_timestamp_parser`: the profile's `cli_timestamp_format` and the
    /// process's local zone.
    pub timestamps: TimestampParser,
    /// The pager chain's inputs (`_construct_cli_pager_chain`); where they
    /// lead depends on whether stdout is a terminal, which the entrypoint
    /// knows.
    pub pager: PagerSettings,
}

/// The pager chain's inputs, owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagerSettings {
    /// `--no-cli-pager` (`no_pager_handler`).
    pub no_cli_pager: bool,
    pub aws_pager: Option<OsString>,
    pub cli_pager: PagerProfile,
    pub pager: Option<OsString>,
}

/// The selected profile's `cli_pager`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PagerProfile {
    Absent,
    Text(String),
    /// A nested block, which Python reads as a dict.
    Block,
}

impl PagerSettings {
    /// The chain `format::pager::destination` reads.
    pub fn chain(&self) -> format::Chain<'_> {
        format::Chain {
            no_cli_pager: self.no_cli_pager,
            aws_pager: self.aws_pager.as_deref(),
            cli_pager: match &self.cli_pager {
                PagerProfile::Absent => format::Profile::Absent,
                PagerProfile::Text(text) => format::Profile::Text(text),
                PagerProfile::Block => format::Profile::Block,
            },
            pager: self.pager.as_deref(),
        }
    }
}

/// What the main parse gave for the output globals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputGlobals<'a> {
    /// `--output`: `None` when not given, `Some(None)` for a value that is
    /// not text.
    pub output: Option<Option<&'a str>>,
    pub no_cli_pager: bool,
    pub profile: Option<&'a str>,
}

impl OutputSettings {
    /// The settings the command prints with, or why only Python prints
    /// them: an output format the native formatters do not write (a value
    /// that is not Unicode or a nested block is none), or a
    /// `cli_timestamp_format` `add_timestamp_parser` rejects.
    pub fn resolve(
        env: &impl EnvMap,
        files: &ParsedFiles,
        globals: &OutputGlobals<'_>,
        local_offset: Option<i64>,
    ) -> Result<Self, PythonReason> {
        let profile = aws_profile::select_or_default(env, globals.profile);
        let name = match (globals.output, env.var_os("AWS_DEFAULT_OUTPUT")) {
            (Some(value), _) => value,
            (None, Some(variable)) => variable.to_str(),
            _ if shared_config::profile_property_is_block(files, profile, "output") => None,
            _ => Some(shared_config::profile_property(files, profile, "output").unwrap_or("json")),
        };
        let format = name
            .and_then(OutputFormat::from_name)
            .ok_or(PythonReason::OutputFormat)?;
        let setting = shared_config::profile_property(files, profile, "cli_timestamp_format");
        let timestamps =
            TimestampFormat::from_setting(setting).map_err(|_| PythonReason::OutputFormat)?;
        let cli_pager = if shared_config::profile_property_is_block(files, profile, "cli_pager") {
            PagerProfile::Block
        } else {
            shared_config::profile_property(files, profile, "cli_pager")
                .map_or(PagerProfile::Absent, |text| {
                    PagerProfile::Text(text.to_owned())
                })
        };
        Ok(Self {
            format,
            timestamps: TimestampParser::new(timestamps, local_offset),
            pager: PagerSettings {
                no_cli_pager: globals.no_cli_pager,
                aws_pager: env.var_os("AWS_PAGER").map(std::ffi::OsStr::to_os_string),
                cli_pager,
                pager: env.var_os("PAGER").map(std::ffi::OsStr::to_os_string),
            },
        })
    }
}

#[cfg(test)]
mod tests;
