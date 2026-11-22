//! Hand-written command rows and the bound command they produce.
use awscli_argparse::{Argument, ParseResult};
use awscli_config::ParsedFiles;
use cli_core::EnvMap;
use cli_core::cli_error::{CommandFailure, PythonReason};
use cli_core::command::Globals;
use std::borrow::Cow;
use std::io::Write;
use std::path::Path;

/// One node of the hand-written command tree: its pinned argparse table,
/// beside the handler in the owning crate, and how it binds.
#[derive(Debug)]
pub struct CommandSpec {
    pub name: &'static str,
    pub arguments: &'static [Argument<'static>],
    pub subcommands: &'static [&'static CommandSpec],
    pub needs: Needs,
    pub streams: StreamPolicy,
    /// `None`: pinned for parsing, with no native handler; the invocation
    /// is Python's.
    pub bind: Option<BindFn>,
}

/// Bind a completed parse to an invocation, or name why Python runs it.
pub type BindFn = fn(&ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission>;

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
    /// Run the command; the status it exits with, or the failure the
    /// error-handler chain renders.
    fn run(&self, cx: &mut Context<'_>) -> Result<u8, CommandFailure>;
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
