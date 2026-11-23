//! The route of a command the registry resolves: an operation of a model
//! source. It meets the stages the pinned `s3`
//! commands meet, in their order: globals, completion, the session's start,
//! binding, settings admission, the output format, and the endpoint checks.
//! The prologue (session fallback, alias rule) ran in the caller.
use super::*;
use ::command::{Needs, Operation, Resolution, StreamPolicy};

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
    let Resolution::Operation(operation) = config.registry.resolve(&names) else {
        return Verdict::Python(PythonReason::UnsupportedSubcommand);
    };
    let target = globals::Target::Streams(operation.streams());
    if let Some(reason) = globals::route_parsed_unsupported_global(result, target) {
        return Verdict::Python(reason);
    }
    // Which of a model operation's options bind natively is the source's
    // to say: a supplied option it defers is named as an option, any other
    // deferred value as a value.
    if awscli_argparse::complete(result, unpack) {
        return Verdict::Python(deferred_reason(result));
    }
    if let Outcome::Failure(failure) = &result.outcome {
        // The leftover check and the unpack loop are the parse's last
        // stages; a table the derivation does not reproduce leaves their
        // failures to Python too.
        if !config.registry.reproduces_failures(&names) {
            return Verdict::Python(PythonReason::UnsupportedSubcommand);
        }
        if result.stage == awscli_argparse::chain::Stage::Unpack
            && operation.precedes_unpack_failure(result)
        {
            return Verdict::Python(PythonReason::UnsupportedValue);
        }
        return session_start(result, config).unwrap_or(Verdict::Failure(failure.clone()));
    }
    let verdict = admit(result, config, operation, unpack.directory);
    if matches!(verdict, Verdict::Run(_))
        && let Some(started) = session_start(result, config)
    {
        return started;
    }
    verdict
}

/// Why the unpack loop stopped: at an option whose every value only Python
/// binds ([`awscli_argparse::CliType::Deferred`]), or at a value native
/// cannot read.
fn deferred_reason(result: &ParseResult<'_>) -> PythonReason {
    let deferred = result.stopped_dest.as_deref().is_some_and(|dest| {
        result.lineage.last().is_some_and(|command| {
            command.arguments.iter().any(|argument| {
                argument.cli_type == Some(awscli_argparse::CliType::Deferred)
                    && argument.matches_dest(dest)
            })
        })
    });
    if deferred {
        PythonReason::UnsupportedOption
    } else {
        PythonReason::UnsupportedValue
    }
}

/// Bind the completed parse, then judge what the bound command needs.
fn admit<E: EnvMap>(
    result: &ParseResult<'_>,
    config: &Configuration<'_, E>,
    operation: Operation<'_>,
    directory: &std::path::Path,
) -> Verdict {
    let invocation = match operation.bind(result) {
        Ok(invocation) => invocation,
        Err(::command::Admission::Python(reason)) => return Verdict::Python(reason),
    };
    let (needs, streams) = (operation.needs(), operation.streams());
    let ns = &result.globals.namespace;
    let globals = bind_globals(ns);
    if let Some(reason) = settings_refusal(config, &globals, ns, streams) {
        return Verdict::Python(reason);
    }
    let Needs::Client { service } = &needs;
    if let Some(reason) = endpoint_refusal(config, &globals, ns, service) {
        return Verdict::Python(reason);
    }
    let command = lineage_names(result).join(".");
    let resolved = client_config::resolve_settings(&client_config::SettingsInputs {
        env: config.env,
        files: config.shared_files,
        globals: &globals,
        directory,
        embedded_trust: config.embedded_trust,
        service,
        command: &command,
    });
    match resolved {
        Ok(settings) => Verdict::Run(Box::new(Run {
            globals,
            invocation,
            needs,
            streams,
            settings,
        })),
        Err(reason) => Verdict::Python(reason),
    }
}

/// The settings stage: the decision table's rows for a model operation's
/// client (`Scope::Service`), then the output format a formatting command
/// would print with. The client's own settings resolve after the endpoint
/// checks, in the s3 commands' order.
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
        scope: effective_config::Scope::Service,
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
