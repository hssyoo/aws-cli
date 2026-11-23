use super::*;
use ::command::StreamPolicy;
use awscli_argparse::chain::Level;
/// How a global is honored, which for the output globals depends on what
/// the command does with its output ([`Target`]).
#[derive(Debug, Clone, Copy)]
enum GlobalSupport {
    Native,
    /// `--output`: a listing ignores it, and a formatting command honors
    /// it, whose format the settings stage judges with the environment's
    /// and the profile's (`generic::output_format_refusal`).
    Output,
    /// `--no-paginate`: a listing ignores it, and a model operation honors
    /// it as `parsed_globals.paginate`, which its binding reads
    /// (`check_should_enable_pagination`, `_make_client_call`); one that
    /// does not paginate ignores it. A transfer is Python's.
    Pagination,
    /// `--color` and `--no-cli-pager`: a command below a root writes no
    /// colored text and starts no pager it would change, or starts one it
    /// honors; a root that only prints its usage failure is Python's.
    Presentation,
    ErrorFormat,
    ConnectTimeout,
    /// `--cli-read-timeout`: a formatting command's client honors it as its
    /// transport's read timeout (`crt::http::Request::read_timeout`); the
    /// CRT S3 client has no such setting, so the s3 commands leave it to
    /// Python.
    ReadTimeout,
    PythonOnly,
}
/// What the command a parse reached does with its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    /// A root with subcommands and no handler of its own.
    Group,
    /// A command, buffered and formatted as its policy says.
    Streams(StreamPolicy),
}
impl Target {
    /// The target of a pinned `s3` lineage: `s3 ls` is a listing and the
    /// other handlers transfer.
    pub(super) fn of_lineage(lineage: NativeLineage) -> Self {
        match lineage {
            NativeLineage::S3 => Self::Group,
            NativeLineage::S3Ls => Self::Streams(StreamPolicy::Listing),
            NativeLineage::S3Cp
            | NativeLineage::S3Mv
            | NativeLineage::S3Rm
            | NativeLineage::S3Sync => Self::Streams(StreamPolicy::Transfer),
        }
    }
}
#[derive(Debug, Clone, Copy)]
struct GlobalClass {
    name: &'static str,
    support: GlobalSupport,
}
impl GlobalClass {
    const fn new(name: &'static str, support: GlobalSupport) -> Self {
        Self { name, support }
    }
}
const GLOBAL_CLASSES: &[GlobalClass] = &[
    GlobalClass::new("debug", GlobalSupport::Native),
    GlobalClass::new("endpoint-url", GlobalSupport::Native),
    GlobalClass::new("no-verify-ssl", GlobalSupport::PythonOnly),
    GlobalClass::new("no-paginate", GlobalSupport::Pagination),
    GlobalClass::new("output", GlobalSupport::Output),
    // Upstream compiles the expression before it ignores it, so an invalid one
    // fails ahead of the command and of any later parse failure; native does
    // not compile JMESPath.
    GlobalClass::new("query", GlobalSupport::PythonOnly),
    GlobalClass::new("profile", GlobalSupport::Native),
    GlobalClass::new("region", GlobalSupport::Native),
    GlobalClass::new("color", GlobalSupport::Presentation),
    GlobalClass::new("no-sign-request", GlobalSupport::Native),
    GlobalClass::new("ca-bundle", GlobalSupport::Native),
    GlobalClass::new("cli-read-timeout", GlobalSupport::ReadTimeout),
    GlobalClass::new("cli-connect-timeout", GlobalSupport::ConnectTimeout),
    GlobalClass::new("cli-binary-format", GlobalSupport::PythonOnly),
    GlobalClass::new("no-cli-pager", GlobalSupport::Presentation),
    GlobalClass::new("cli-auto-prompt", GlobalSupport::PythonOnly),
    GlobalClass::new("no-cli-auto-prompt", GlobalSupport::PythonOnly),
    GlobalClass::new("cli-error-format", GlobalSupport::ErrorFormat),
    GlobalClass::new("help", GlobalSupport::PythonOnly),
];
const _: () = {
    let table = awscli_argparse::tables::GLOBALS;
    let mut i = 0;
    while i < table.len() {
        if !matches!(table[i].action, awscli_argparse::Action::Version) {
            assert!(
                classify_global(table[i].name).is_some(),
                "pinned global is unclassified"
            );
        }
        i += 1;
    }
    assert!(
        table.len() == GLOBAL_CLASSES.len() + 1,
        "global classifications are stale"
    );
};
const fn classify_global(name: &str) -> Option<GlobalSupport> {
    let mut i = 0;
    while i < GLOBAL_CLASSES.len() {
        if same(GLOBAL_CLASSES[i].name, name) {
            return Some(GLOBAL_CLASSES[i].support);
        }
        i += 1;
    }
    None
}
const fn same(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a.as_bytes()[i] != b.as_bytes()[i] {
            return false;
        }
        i += 1;
    }
    true
}

#[expect(
    clippy::expect_used,
    reason = "const table audit proves every global is classified"
)]
pub(super) fn route_parsed_unsupported_global(
    result: &ParseResult<'_>,
    target: Target,
) -> Option<PythonReason> {
    let level = &result.globals;
    level
        .table
        .iter()
        .zip(&level.seen)
        .any(|(argument, &seen)| {
            if !seen || argument.positional || argument.action == awscli_argparse::Action::Version {
                return false;
            }
            let support = classify_global(argument.name).expect("all pinned globals classified");
            !is_native_global(support, target, level)
        })
        .then_some(PythonReason::PythonOnlyGlobal)
}
fn is_native_global(support: GlobalSupport, target: Target, level: &Level<'_>) -> bool {
    let streams = match target {
        Target::Group => None,
        Target::Streams(policy) => Some(policy),
    };
    match support {
        GlobalSupport::Native => true,
        GlobalSupport::Output => matches!(
            streams,
            Some(StreamPolicy::Listing | StreamPolicy::Formatted)
        ),
        GlobalSupport::Pagination => matches!(
            streams,
            Some(StreamPolicy::Listing | StreamPolicy::Formatted)
        ),
        GlobalSupport::Presentation => streams.is_some(),
        GlobalSupport::ErrorFormat => level
            .namespace
            .string("cli_error_format")
            .and_then(ErrorFormat::from_name)
            .is_some(),
        GlobalSupport::ConnectTimeout => {
            let timeout = bind_timeout(&level.namespace, "connect_timeout");
            client_config::timeout_milliseconds(timeout).is_some()
        }
        GlobalSupport::ReadTimeout => {
            let timeout = bind_timeout(&level.namespace, "read_timeout");
            streams == Some(StreamPolicy::Formatted)
                && client_config::timeout_milliseconds(timeout).is_some()
        }
        GlobalSupport::PythonOnly => false,
    }
}
