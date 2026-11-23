//! What a registry command needs, and the bound command it produces.
use awscli_config::ParsedFiles;
use cli_core::EnvMap;
use cli_core::cli_error::{CommandFailure, PythonReason};
use cli_core::command::Globals;
use std::borrow::Cow;
use std::io::Write;
use std::path::Path;

/// What the router must resolve before the command may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Needs {
    /// A client for a service. `service` is the identity botocore's
    /// configured-endpoint lookup keys on (`ConfiguredEndpointProvider`):
    /// the model's `serviceId`, hyphenated and lowercased, which names the
    /// `AWS_ENDPOINT_URL_<SERVICE>` variable and the `services` section's
    /// entry.
    Client { service: Cow<'static, str> },
}

/// How the command's standard streams are buffered, and so which output
/// globals it honors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPolicy {
    /// The response goes through the `--output` formatter
    /// (`CLIOperationCaller._display_response`), as every model operation's
    /// does.
    Formatted,
    /// A listing writing its own lines, which ignores the output globals,
    /// as `s3 ls` does (`ListCommand`).
    Listing,
    /// A transfer writing its own progress and results, as `s3 cp`, `mv`,
    /// `rm` and `sync` do (`S3TransferCommand`).
    Transfer,
}

/// Why a parsed command does not run natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Python runs the untouched argv, for this reason.
    Python(PythonReason),
}

/// A bound, admitted command: everything argv decided, ready to run.
pub trait Invocation: std::fmt::Debug {
    /// What the command decides from the process's settings before any
    /// native runtime starts, any request is sent or anything is written:
    /// a reason the invocation is Python's after all. It runs once, before
    /// [`Invocation::run`].
    fn preflight(&self, _cx: &Preflight<'_>) -> Result<(), PythonReason> {
        Ok(())
    }
    /// Run the command; the status it exits with, or the failure the
    /// error-handler chain renders.
    fn run(&self, cx: &mut Context<'_>) -> Result<u8, CommandFailure>;
}

/// What an invocation's preflight reads.
pub struct Preflight<'a> {
    pub env: &'a cli_core::SnapshotEnv,
    /// The process's environment, as the unpack loop and the pager read it.
    pub variables: &'a std::collections::HashMap<String, std::ffi::OsString>,
    pub shared_files: &'a ParsedFiles,
    pub globals: &'a Globals,
    pub directory: &'a Path,
    /// The settings the router resolved for the command's client.
    pub settings: &'a cli_core::client_config::ClientSettings,
    /// How the response prints, which the router resolved.
    pub output: &'a crate::OutputSettings,
    pub stdout_is_tty: bool,
}
impl std::fmt::Debug for Preflight<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Preflight")
            .field("globals", self.globals)
            .field("stdout_is_tty", &self.stdout_is_tty)
            .finish_non_exhaustive()
    }
}

/// What an invocation may read and write as it runs.
pub struct Context<'a> {
    pub env: &'a dyn EnvMap,
    pub shared_files: &'a ParsedFiles,
    pub globals: &'a Globals,
    pub directory: &'a Path,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}
impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("globals", self.globals)
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}
