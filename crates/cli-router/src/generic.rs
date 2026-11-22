//! The route of a command the registry resolves: a hand-written spec or an
//! operation of a model source. It meets the stages the pinned `s3`
//! commands meet, in their order: globals, completion, the session's start,
//! binding, settings admission, the output format, and the endpoint checks.
//! The prologue (session fallback, alias rule) ran in the caller.
use super::*;
use ::command::{CommandSpec, Needs, Operation, Resolution, StreamPolicy};

/// What binds the lineage the parse reached.
#[derive(Debug, Clone, Copy)]
enum Bound<'r> {
    Spec(&'static CommandSpec),
    Operation(Operation<'r>),
    /// A hand-written root with subcommands and no handler, whose bare
    /// parse is its usage failure.
    Group,
}
impl Bound<'_> {
    fn target(&self) -> globals::Target {
        match self {
            Self::Spec(spec) => globals::Target::Streams(spec.streams),
            Self::Operation(operation) => globals::Target::Streams(operation.streams()),
            Self::Group => globals::Target::Group,
        }
    }
}

/// [`route_parsed`] for a lineage the pinned enum path does not bind.
pub(super) fn route<E: EnvMap>(
    result: &mut ParseResult<'_>,
    unpack: &unpack::Environment<'_>,
    config: &Configuration<'_, E>,
) -> Verdict {
    let owned: Vec<String> = result
        .lineage
        .iter()
        .map(|node| node.name.to_string())
        .collect();
    let names: Vec<&str> = owned.iter().map(String::as_str).collect();
    let bound = match config.registry.resolve(&names) {
        Resolution::Spec(spec) if spec.bind.is_some() => Bound::Spec(spec),
        Resolution::Spec(spec) if !spec.subcommands.is_empty() => Bound::Group,
        Resolution::Operation(operation) => Bound::Operation(operation),
        Resolution::Spec(_) | Resolution::Unknown => {
            return Verdict::Python(PythonReason::UnsupportedSubcommand);
        }
    };
    if let Some(reason) = globals::route_parsed_unsupported_global(result, bound.target()) {
        return Verdict::Python(reason);
    }
    // Which of a model operation's options bind natively is the source's
    // to say, so a value deferred to Python is named as a value.
    if awscli_argparse::complete(result, unpack) {
        return Verdict::Python(PythonReason::UnsupportedValue);
    }
    if let Outcome::Failure(failure) = &result.outcome {
        return session_start(result, config).unwrap_or(Verdict::Failure(failure.clone()));
    }
    let verdict = admit(result, config, bound);
    if matches!(verdict, Verdict::Run(_))
        && let Some(started) = session_start(result, config)
    {
        return started;
    }
    verdict
}

/// Bind the completed parse, then judge what the bound command needs.
fn admit<E: EnvMap>(
    result: &ParseResult<'_>,
    config: &Configuration<'_, E>,
    bound: Bound<'_>,
) -> Verdict {
    let bind = match bound {
        Bound::Spec(CommandSpec {
            bind: Some(bind),
            needs,
            streams,
            ..
        }) => bind(result).map(|invocation| (invocation, needs.clone(), *streams)),
        Bound::Operation(operation) => operation
            .bind(result)
            .map(|invocation| (invocation, operation.needs(), operation.streams())),
        Bound::Spec(_) | Bound::Group => {
            return Verdict::Python(PythonReason::UnsupportedSubcommand);
        }
    };
    let (invocation, needs, streams) = match bind {
        Ok(bound) => bound,
        Err(::command::Admission::Python(reason)) => return Verdict::Python(reason),
    };
    let ns = &result.globals.namespace;
    let globals = bind_globals(ns);
    if let Some(reason) = settings_refusal(config, &globals, ns, streams) {
        return Verdict::Python(reason);
    }
    let Needs::Client { service } = &needs;
    if let Some(reason) = endpoint_refusal(config, &globals, ns, service) {
        return Verdict::Python(reason);
    }
    Verdict::Run(Box::new(Run {
        globals,
        invocation,
        needs,
        streams,
    }))
}

/// The settings stage: the decision table, which no registry command reads
/// with a transfer manager, then the output format a formatting command
/// would print with.
fn settings_refusal<E: EnvMap>(
    config: &Configuration<'_, E>,
    globals: &Globals,
    ns: &Namespace,
    streams: StreamPolicy,
) -> Option<PythonReason> {
    let refused = effective_config::admit(&effective_config::Inputs {
        env: config.env,
        shared_files: config.shared_files,
        cli_profile: globals.profile.as_deref(),
        cli_error_format: ns.string("cli_error_format"),
        transfers: false,
        boto_config_credentials: config.boto_config_credentials,
    });
    if refused.is_some() {
        return refused;
    }
    (streams == StreamPolicy::Formatted)
        .then(|| output_format_refusal(config, globals, ns))
        .flatten()
}

/// [`PythonReason::OutputFormat`] unless the format
/// `CLIOperationCaller._display_response` hands `get_formatter` is one the
/// native formatters write. `CLIDriver._construct_cli_output_chain` reads
/// `--output`, then `AWS_DEFAULT_OUTPUT` whenever it is set, then the
/// selected profile's `output`, then `json`. A value that is not Unicode,
/// or a nested block, which botocore returns as a dictionary, is no format
/// native writes.
fn output_format_refusal<E: EnvMap>(
    config: &Configuration<'_, E>,
    globals: &Globals,
    ns: &Namespace,
) -> Option<PythonReason> {
    use cli_core::shared_config::{profile_property, profile_property_is_block};
    let files = config.shared_files;
    let profile = cli_core::aws_profile::select_or_default(config.env, globals.profile.as_deref());
    let format = match (ns.value("output"), config.env.var_os("AWS_DEFAULT_OUTPUT")) {
        (Some(value), _) if !value.is_null() => value.as_str(),
        (_, Some(variable)) => variable.to_str(),
        _ if profile_property_is_block(files, profile, "output") => None,
        _ => Some(profile_property(files, profile, "output").unwrap_or("json")),
    };
    (!matches!(format, Some("json" | "text" | "off"))).then_some(PythonReason::OutputFormat)
}
