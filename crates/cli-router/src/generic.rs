//! The route of a command the registry resolves: an operation of a model
//! source. It meets the stages the pinned `s3`
//! commands meet, in their order: globals, completion, the session's start,
//! binding, settings admission, the output format, and the endpoint checks.
//! The prologue (session fallback, alias rule) ran in the caller.
use super::*;
use ::command::{Operation, Resolution};

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
    let target = globals::Target::Operation;
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
        if let Some(reason) = globals::query_precedes_failure(result) {
            return Verdict::Python(reason);
        }
        // The leftover check and the unpack loop are the parse's last
        // stages; a table the derivation does not reproduce leaves their
        // failures to Python too.
        if !config.registry.reproduces_failures(&names) {
            return Verdict::Python(PythonReason::UnsupportedSubcommand);
        }
        if result.stage == awscli_argparse::chain::Stage::Unpack
            && operation.precedes_unpack_failure(result)
        {
            return Verdict::Python(PythonReason::PagingHandler);
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
    let ::command::Bound { invocation, needs } = match operation.bind(result) {
        Ok(bound) => bound,
        Err(::command::Admission::Python(reason)) => return Verdict::Python(reason),
    };
    let streams = operation.streams();
    // The `s3` block's settings are honored only on the s3 commands' path;
    // the rows of `Scope::Service` assume a client that reads none.
    if needs.reads_s3_settings {
        return Verdict::Python(PythonReason::EndpointSetting);
    }
    let ns = &result.globals.namespace;
    let globals = bind_globals(ns);
    let output = match settings(config, &globals, ns) {
        Ok(output) => output,
        Err(reason) => return Verdict::Python(reason),
    };
    let service = &needs.service;
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
            output,
        })),
        Err(reason) => Verdict::Python(reason),
    }
}

/// The settings stage: the decision table's rows for a model operation's
/// client (`Scope::Service`), then how its response prints, resolved once
/// ([`::command::OutputSettings`]). The client's own settings resolve after
/// the endpoint checks, in the s3 commands' order.
fn settings<E: EnvMap>(
    config: &Configuration<'_, E>,
    globals: &Globals,
    ns: &Namespace,
) -> Result<::command::OutputSettings, PythonReason> {
    let refused = effective_config::admit(&effective_config::Inputs {
        env: config.env,
        shared_files: config.shared_files,
        cli_profile: globals.profile.as_deref(),
        cli_error_format: ns.string("cli_error_format"),
        scope: effective_config::Scope::Service,
        boto_config_credentials: config.boto_config_credentials,
    });
    if let Some(reason) = refused {
        return Err(reason);
    }
    let output = ns
        .value("output")
        .filter(|value| !value.is_null())
        .map(|value| value.as_str());
    let local_offset =
        cli_core::local_zone::LocalZone::system().and_then(|zone| zone.fixed_offset());
    ::command::OutputSettings::resolve(
        config.env,
        config.shared_files,
        &::command::OutputGlobals {
            output,
            no_cli_pager: ns.boolean("no_cli_pager"),
            profile: globals.profile.as_deref(),
        },
        local_offset,
    )
}
