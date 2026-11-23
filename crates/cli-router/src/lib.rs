//! Top-level AWS CLI router: turns one complete parse of argv, plus the
//! configuration in force, into the verdict for the whole invocation before
//! any side effect. An invocation runs natively if and only if every part of
//! it is natively supported: the parse outcome, the globals, the command,
//! its options and values, its operands, and the settings the environment
//! and the selected profile resolve. A failure the AWS CLI would report
//! before running anything is rendered natively once its text is settled;
//! everything else goes, untouched, to the Python fallback.
#![forbid(unsafe_code)]
use awscli_argparse::{
    Namespace, ParseResult,
    chain::{Outcome, Stage},
    failure::Failure,
    unpack,
};
use awscli_config::{File as ConfigFile, ParsedFiles, Status};
use cli_core::client_config;
use cli_core::{EnvMap, cli_error::ErrorFormat};
#[doc(inline)]
pub use cli_core::{cli_error::PythonReason, command::Globals};
pub mod effective_config;
mod generic;
mod globals;

/// The native command and its owned, checked arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    S3Ls(s3::ls::Args),
    S3Cp(Box<s3::cp_args::Args>),
    S3Mv(s3::mv::Args),
    S3Rm(s3::rm::Args),
    S3Sync(Box<s3::sync::Args>),
}
impl Command {
    /// How the handler's streams are buffered: `s3 ls` lists, and the other
    /// handlers transfer.
    pub fn streams(&self) -> ::command::StreamPolicy {
        match self {
            Self::S3Ls(_) => ::command::StreamPolicy::Listing,
            Self::S3Cp(_) | Self::S3Mv(_) | Self::S3Rm(_) | Self::S3Sync(_) => {
                ::command::StreamPolicy::Transfer
            }
        }
    }
    /// The record command whose client settings the handler runs with.
    pub fn client_command(&self) -> client_config::Command {
        match self {
            Self::S3Ls(_) => client_config::Command::Ls,
            Self::S3Cp(_) => client_config::Command::Cp,
            Self::S3Mv(_) => client_config::Command::Mv,
            Self::S3Rm(_) => client_config::Command::Rm,
            Self::S3Sync(_) => client_config::Command::Sync,
        }
    }
}
/// The dispatch payload of a native verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Native {
    /// Global overrides shared by the native handlers.
    pub globals: Globals,
    /// Selected handler and the arguments admitted for native execution.
    pub command: Command,
    /// The client settings the handler runs with, resolved once.
    pub client: client_config::ClientConfig,
}
/// The dispatch payload of a registry command's verdict: a model operation
/// the registry's binder bound, admitted by every stage.
#[derive(Debug)]
pub struct Run {
    /// Global overrides shared by every native command.
    pub globals: Globals,
    /// The bound command.
    pub invocation: Box<dyn ::command::Invocation>,
    /// What the command needs before it runs; the stages checked it.
    pub needs: ::command::Needs,
    /// How the command's streams are buffered.
    pub streams: ::command::StreamPolicy,
    /// The settings the command's client runs with, resolved once.
    pub settings: client_config::ClientSettings,
    /// How the command's response prints, resolved once.
    pub output: ::command::OutputSettings,
}
/// The router's decision. A failure is an owned copy of the parse result's.
/// `Native` is a pinned `s3` command of the router's own enum path; `Run`
/// is a command the registry resolves.
#[derive(Debug)]
pub enum Verdict {
    Native(Box<Native>),
    Run(Box<Run>),
    Failure(Failure),
    Python(PythonReason),
}
/// What the configuration stage reads: the environment and the shared
/// config, credentials, and alias files parsed once before routing.
#[derive(Debug)]
pub struct Configuration<'a, E: EnvMap> {
    /// Borrowed process environment used to resolve effective settings.
    pub env: &'a E,
    /// Borrowed shared files used to resolve the selected profile.
    pub shared_files: &'a ParsedFiles,
    /// Whether a boto config file supplies credentials botocore would read
    /// (`shared_config::boto_config_supplies_credentials`). The entrypoint
    /// reads the host's files; tests state the answer.
    pub boto_config_credentials: bool,
    /// Whether the payload embeds botocore's CA bundle, which a native
    /// client then trusts when nothing configures another
    /// ([`client_config::Inputs::embedded_trust`]). The entrypoint asks the
    /// payload; tests state the answer.
    pub embedded_trust: bool,
    /// The roots the parse resolved its commands through, which resolve and
    /// bind the lineage it reached.
    pub registry: &'a ::command::Registry<'a>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeLineage {
    S3,
    S3Ls,
    S3Cp,
    S3Mv,
    S3Rm,
    S3Sync,
}
/// Development override for automatic, Python-only, or native-only routing.
pub const ROUTING_ENV_VAR: &str = "AWS_CLI_NATIVE_ROUTER";
/// Whether Python fallback is automatic, required, or forbidden.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RoutingMode {
    #[default]
    Auto,
    Native,
    Python,
}
/// Unset and unknown values preserve automatic routing for compatibility.
pub fn routing_mode(value: Option<&str>) -> RoutingMode {
    match value {
        Some("native") => RoutingMode::Native,
        Some("python") => RoutingMode::Python,
        _ => RoutingMode::Auto,
    }
}
/// The reason of a run [`routing_mode`] sends to Python whatever its argv;
/// `None` otherwise.
pub fn forced_route(value: Option<&str>) -> Option<PythonReason> {
    (routing_mode(value) == RoutingMode::Python).then_some(PythonReason::EnvOverride)
}

/// Decide the invocation from its parse and the configuration in force,
/// running the chain's value-processing and leftover stages on the way. The
/// rules apply in the order the AWS CLI would surface them, so a native
/// failure is always the one Python would have reported first; a Python
/// verdict is always safe because the fallback repeats the whole parse from
/// the original argv. The configuration stage runs last, after every
/// argv-only rule and before the native verdict, so a setting the native
/// path cannot honor is caught before any request or mutation. Parameter
/// files are read through `unpack`, relative to its directory. The platform
/// facts are the default record, a host the CRT knows nothing about, and the
/// CRT process lock is one no other process holds, so routing stays
/// deterministic; [`route_with_process_lock`] takes the host's probe and lock.
pub fn route<E: EnvMap>(
    result: &mut ParseResult<'_>,
    unpack: &unpack::Environment<'_>,
    config: &Configuration<'_, E>,
) -> Verdict {
    route_with_platform(result, unpack, config, client_config::Platform::default())
}

/// Resolve the command using caller-supplied platform facts; no probe runs in tests.
pub fn route_with_platform<E: EnvMap>(
    result: &mut ParseResult<'_>,
    unpack: &unpack::Environment<'_>,
    config: &Configuration<'_, E>,
    platform: client_config::Platform,
) -> Verdict {
    route_with_probe(result, unpack, config, || platform)
}

/// [`route`], probing the host only after routing and resolved settings
/// require platform facts. The CRT process lock is one no other process
/// holds, so routing stays deterministic.
pub fn route_with_probe<E: EnvMap>(
    result: &mut ParseResult<'_>,
    unpack: &unpack::Environment<'_>,
    config: &Configuration<'_, E>,
    probe: impl FnOnce() -> client_config::Platform,
) -> Verdict {
    route_with_process_lock(result, unpack, config, probe, process_lock_uncontended)
}

/// A CRT process lock no other process holds, taken without side effects:
/// the lock [`route`] and [`route_with_probe`] route under, and the one a
/// build without the CRT supplies.
pub fn process_lock_uncontended() -> bool {
    true
}

/// [`route_with_probe`], taking the AWS CLI's CRT process lock through
/// `acquire_process_lock`, which reports whether this process holds it, only
/// for a transfer command the CRT client runs. The entrypoint supplies the
/// CRT's own lock, which the process keeps while it runs.
///
/// Before any outcome is examined, the prologue applies the order in which
/// the AWS CLI reads its files: create_clidriver runs the first-pass parse
/// and then reads the shared config and credentials files, where a malformed
/// file is a `ConfigParseError`, and loads the plugins the config file
/// configures; resolve_auto_prompt_mode raises the auto-prompt conflict next;
/// CLIDriver.main then loads the alias file before its main parse. A failure
/// raised before a file is read renders natively whatever the file holds.
pub fn route_with_process_lock<E: EnvMap>(
    result: &mut ParseResult<'_>,
    unpack: &unpack::Environment<'_>,
    config: &Configuration<'_, E>,
    probe: impl FnOnce() -> client_config::Platform,
    acquire_process_lock: impl FnOnce() -> bool,
) -> Verdict {
    let files = config.shared_files;
    if let Some(failure) = early_failure(result, Stage::FirstPass) {
        return Verdict::Failure(failure);
    }
    if let Some(reason) = session_fallback(files) {
        return Verdict::Python(reason);
    }
    if let Some(failure) = early_failure(result, Stage::AutoPrompt) {
        return Verdict::Failure(failure);
    }
    if unusable_alias_file(&files.aliases) {
        return Verdict::Python(PythonReason::ConfigParseFailure);
    }
    match &result.outcome {
        Outcome::Version => Verdict::Python(PythonReason::VersionRequested),
        Outcome::UnsupportedOsValue => Verdict::Python(PythonReason::UnsupportedValue),
        Outcome::Help => Verdict::Python(PythonReason::HelpRequested),
        Outcome::Unresolved { .. } => Verdict::Python(PythonReason::UnsupportedCommand),
        Outcome::Failure(failure) => route_failure(result, failure.clone(), config),
        Outcome::Parsed => route_parsed(result, unpack, config, probe, acquire_process_lock),
    }
}
fn early_failure(result: &ParseResult<'_>, stage: Stage) -> Option<Failure> {
    assert!(matches!(stage, Stage::FirstPass | Stage::AutoPrompt));
    if result.stage != stage {
        return None;
    }
    match &result.outcome {
        Outcome::Failure(failure) => Some(failure.clone()),
        _ => None,
    }
}
fn route_failure<E: EnvMap>(
    result: &ParseResult<'_>,
    failure: Failure,
    config: &Configuration<'_, E>,
) -> Verdict {
    let files = config.shared_files;
    let names = lineage_names(result);
    let reason = match result.stage {
        Stage::AutoPrompt | Stage::FirstPass => None,
        Stage::Main => result
            .failed
            .as_ref()
            .and_then(|n| n.string("command"))
            .filter(|c| !config.registry.is_root(c))
            .map(|_| PythonReason::UnsupportedCommand),
        _ if names.first().is_some_and(|root| defines_alias(files, root)) => {
            Some(PythonReason::CommandAlias)
        }
        _ if native_lineage(result).is_none() && !config.registry.reproduces_failures(&names) => {
            Some(PythonReason::UnsupportedSubcommand)
        }
        _ => None,
    };
    if let Some(reason) = reason {
        return Verdict::Python(reason);
    }
    matches!(result.stage, Stage::Probe | Stage::Table)
        .then(|| session_start(result, config))
        .flatten()
        .unwrap_or(Verdict::Failure(failure))
}
/// What the AWS CLI does as its session starts, where that comes ahead of
/// an outcome the native side would report itself: a command's own failure,
/// or its run. `CLIDriver.main` handles the top-level arguments, then emits
/// `session-initialized`, and only then hands the command its arguments.
///
/// One handler of that event has botocore build its credential chain
/// (`inject_assume_role_provider_cache`), which refuses the instance metadata
/// settings it cannot use (`create_credential_resolver`): for every command,
/// whichever provider would answer, and for a command that signs nothing.
/// The refusal is rendered natively, as the general handler renders it.
fn session_start<E: EnvMap>(
    result: &ParseResult<'_>,
    config: &Configuration<'_, E>,
) -> Option<Verdict> {
    let (env, files) = (config.env, config.shared_files);
    let cli_profile = result.globals.namespace.string("profile");
    // A selected profile without a section is botocore's ProfileNotFound,
    // which is what is reported then.
    let profile = match cli_core::aws_profile::select(env, cli_profile) {
        Some(name) if !cli_core::aws_profile::explicit_profile_exists(files, name) => return None,
        Some(name) => name.to_str()?,
        None => "default",
    };
    let refusal = s3::credentials::settings_refusal(env, files, profile)?;
    let kind = awscli_argparse::failure::FailureKind::SessionInitialized(refusal);
    Some(Verdict::Failure(Failure::new(kind)))
}
fn route_parsed<E: EnvMap>(
    result: &mut ParseResult<'_>,
    unpack: &unpack::Environment<'_>,
    config: &Configuration<'_, E>,
    probe: impl FnOnce() -> client_config::Platform,
    acquire_process_lock: impl FnOnce() -> bool,
) -> Verdict {
    let root = lineage_names(result).first().map(|root| (*root).to_owned());
    if root.is_some_and(|root| defines_alias(config.shared_files, &root)) {
        return Verdict::Python(PythonReason::CommandAlias);
    }
    let Some(lineage) = native_lineage(result) else {
        return generic::route(result, unpack, config);
    };
    let target = globals::Target::of_lineage(lineage);
    if let Some(reason) = globals::route_parsed_unsupported_global(result, target) {
        return Verdict::Python(reason);
    }
    if awscli_argparse::complete(result, unpack) {
        let dest = result.stopped_dest.as_deref();
        return Verdict::Python(route_parsed_unsupported_reason(lineage, dest));
    }
    if let Outcome::Failure(failure) = &result.outcome {
        return session_start(result, config).unwrap_or(Verdict::Failure(failure.clone()));
    }
    if let Some(reason) = route_parsed_unsupported_option(result, lineage) {
        return Verdict::Python(reason);
    }
    let verdict = route_admitted(result, config, lineage, unpack.directory, probe);
    if let Verdict::Native(native) = &verdict {
        if let Some(started) = session_start(result, config) {
            return started;
        }
        if let Some(reason) = acquire_crt_client_lock(&native.client, acquire_process_lock) {
            return Verdict::Python(reason);
        }
    }
    verdict
}
/// Takes the AWS CLI's CRT process lock for the CRT client `s3 cp`, `s3 mv`,
/// `s3 rm`, and `s3 sync` run, as upstream takes it for every CRT client it builds, while
/// `s3 ls`, which builds none, takes nothing; the reason when another AWS CLI
/// process holds it and that sends upstream to its classic client. Under
/// `auto` a lock upstream cannot take resolves the command to the classic
/// client; a preferred CRT client runs whoever holds it
/// (`TransferManagerFactory`). Settings the CRT client cannot honor were sent
/// to Python before this, as upstream checks them before the lock.
fn acquire_crt_client_lock(
    client: &client_config::ClientConfig,
    acquire_process_lock: impl FnOnce() -> bool,
) -> Option<PythonReason> {
    let persona = client.persona.as_ref()?.value;
    if acquire_process_lock() {
        return None;
    }
    match persona {
        client_config::Persona::CrtPreferred => None,
        client_config::Persona::Tuned | client_config::Persona::NewlyEligible => {
            Some(PythonReason::CrtClientRunning)
        }
    }
}
fn route_admitted<E: EnvMap>(
    result: &ParseResult<'_>,
    config: &Configuration<'_, E>,
    lineage: NativeLineage,
    directory: &std::path::Path,
    probe: impl FnOnce() -> client_config::Platform,
) -> Verdict {
    let ns = &result.globals.namespace;
    let globals = bind_globals(ns);
    let (command, reason) = bind_command(result, lineage);
    let client_command = command.client_command();
    if let Some(reason) = reason {
        return Verdict::Python(match reason {
            s3::AdmissionReason::UnsupportedValue => PythonReason::UnsupportedValue,
            s3::AdmissionReason::UnsupportedOperand => PythonReason::UnsupportedOperand,
        });
    }
    let configured = effective_config::admit(&effective_config::Inputs {
        env: config.env,
        shared_files: config.shared_files,
        cli_profile: globals.profile.as_deref(),
        cli_error_format: ns.string("cli_error_format"),
        scope: if client_command.transfers() {
            effective_config::Scope::Transfer
        } else {
            effective_config::Scope::S3
        },
        boto_config_credentials: config.boto_config_credentials,
    });
    if let Some(reason) = configured {
        return Verdict::Python(reason);
    }
    if let Some(reason) = endpoint_refusal(config, &globals, ns, S3_SERVICE) {
        return Verdict::Python(reason);
    }
    let resolved = cli_core::client_config::resolve_with_probe(
        &cli_core::client_config::Inputs {
            env: config.env,
            files: config.shared_files,
            globals: &globals,
            command: client_command,
            directory,
            platform: client_config::Platform::default(),
            embedded_trust: config.embedded_trust,
        },
        probe,
    );
    let client = match resolved {
        Ok(client) => client,
        Err(reason) => return Verdict::Python(reason),
    };
    Verdict::Native(Box::new(Native {
        globals,
        command,
        client,
    }))
}
#[expect(
    clippy::expect_used,
    reason = "completion proves the const-checked native schema"
)]
#[expect(
    clippy::unreachable,
    reason = "root completion always yields a usage failure"
)]
fn bind_command(
    result: &ParseResult<'_>,
    lineage: NativeLineage,
) -> (Command, Option<s3::AdmissionReason>) {
    match lineage {
        NativeLineage::S3Ls => {
            let args = s3::ls::Args::from_result(result).expect("completed ls binding");
            let reason = s3::ls::admit(&args);
            (Command::S3Ls(args), reason)
        }
        NativeLineage::S3Cp => {
            let args = s3::cp_args::Args::from_result(result).expect("completed cp binding");
            let reason = s3::cp_args::admit(&args);
            (Command::S3Cp(Box::new(args)), reason)
        }
        NativeLineage::S3Mv => {
            let args = s3::mv::Args::from_result(result).expect("completed mv binding");
            let reason = s3::mv::admit(&args);
            (Command::S3Mv(args), reason)
        }
        NativeLineage::S3Rm => {
            let args = s3::rm::Args::from_result(result).expect("completed rm binding");
            let reason = s3::rm::admit(&args);
            (Command::S3Rm(args), reason)
        }
        NativeLineage::S3Sync => {
            let args = s3::sync::Args::from_result(result).expect("completed sync binding");
            let reason = s3::sync::admit(&args);
            (Command::S3Sync(Box::new(args)), reason)
        }
        NativeLineage::S3 => unreachable!("bare s3 completes with usage failure"),
    }
}
/// Whether botocore fails to create the command's client, with an error only
/// the fallback renders: the region its chain resolves is empty, for which it
/// builds no valid endpoint, or no valid host label, which its session
/// refuses (`region::is_valid_name`, `Session._resolve_region_name`); or
/// botocore rejects the selected endpoint, the command line's included,
/// before any request (`endpoint_config::configured_endpoint_fails`). The command line's URL is
/// `cli_endpoint` as given, which global resolution passes with a scheme even
/// when it is not Unicode.
fn client_creation_fails<E: EnvMap>(
    config: &Configuration<'_, E>,
    globals: &Globals,
    cli_endpoint: Option<&std::ffi::OsStr>,
    service: &str,
) -> bool {
    let region = cli_core::region::session_region(
        config.env,
        config.shared_files,
        globals.region.as_deref(),
        globals.profile.as_deref(),
    );
    if region.is_some_and(|name| name.is_empty() || !cli_core::region::is_valid_name(&name)) {
        return true;
    }
    cli_core::endpoint_config::configured_endpoint_fails(
        config.env,
        config.shared_files,
        service,
        cli_endpoint,
        globals.profile.as_deref(),
    )
}
fn selected_endpoint_resolves_natively<E: EnvMap>(
    config: &Configuration<'_, E>,
    globals: &Globals,
    service: &str,
) -> bool {
    cli_core::endpoint_config::selected_endpoint_resolves_natively(
        config.env,
        config.shared_files,
        service,
        globals.endpoint_url.as_deref(),
        globals.profile.as_deref(),
    )
}
/// The endpoint stage for a client of `service`: [`client_creation_fails`],
/// then [`selected_endpoint_resolves_natively`].
fn endpoint_refusal<E: EnvMap>(
    config: &Configuration<'_, E>,
    globals: &Globals,
    ns: &Namespace,
    service: &str,
) -> Option<PythonReason> {
    let refused = client_creation_fails(config, globals, ns.os_string("endpoint_url"), service)
        || !selected_endpoint_resolves_natively(config, globals, service);
    refused.then_some(PythonReason::EndpointSetting)
}
/// The pinned `s3` commands' client service, which the enum path's
/// endpoint checks name.
const S3_SERVICE: &str = "s3";
/// The lineage's command names, outermost first.
fn lineage_names<'r>(result: &'r ParseResult<'_>) -> Vec<&'r str> {
    result.lineage.iter().map(|node| &*node.name).collect()
}
fn bind_globals(ns: &Namespace) -> Globals {
    Globals {
        debug: ns.boolean("debug"),
        region: ns.string("region").map(str::to_owned),
        profile: ns.string("profile").map(str::to_owned),
        endpoint_url: ns.string("endpoint_url").map(str::to_owned),
        ca_bundle: ns.string("ca_bundle").map(str::to_owned),
        no_sign_request: !ns.boolean("sign_request"),
        connect_timeout: bind_timeout(ns, "connect_timeout"),
        read_timeout: bind_timeout(ns, "read_timeout"),
    }
}
/// A timeout global as global resolution leaves it: `None` when the command
/// line did not set it, else its seconds, with 0 for the null that an
/// explicit 0 resolves to (globalargs.py `_resolve_timeout`).
fn bind_timeout(ns: &Namespace, dest: &str) -> Option<i64> {
    ns.is_explicit(dest).then(|| ns.integer(dest).unwrap_or(0))
}

fn native_lineage(result: &ParseResult<'_>) -> Option<NativeLineage> {
    match lineage_names(result).as_slice() {
        [S3_SERVICE] => Some(NativeLineage::S3),
        [S3_SERVICE, "ls"] => Some(NativeLineage::S3Ls),
        [S3_SERVICE, "cp"] => Some(NativeLineage::S3Cp),
        [S3_SERVICE, "mv"] => Some(NativeLineage::S3Mv),
        [S3_SERVICE, "rm"] => Some(NativeLineage::S3Rm),
        [S3_SERVICE, "sync"] => Some(NativeLineage::S3Sync),
        _ => None,
    }
}
fn route_parsed_unsupported_option(
    result: &ParseResult<'_>,
    lineage: NativeLineage,
) -> Option<PythonReason> {
    result
        .args
        .namespace
        .entries
        .iter()
        .any(|entry| entry.explicit && !is_native_dest(lineage, &entry.dest))
        .then_some(PythonReason::UnsupportedOption)
}
fn route_parsed_unsupported_reason(lineage: NativeLineage, dest: Option<&str>) -> PythonReason {
    if dest.is_some_and(|dest| is_native_dest(lineage, dest)) {
        PythonReason::UnsupportedValue
    } else {
        PythonReason::UnsupportedOption
    }
}
fn is_native_dest(lineage: NativeLineage, dest: &str) -> bool {
    match lineage {
        NativeLineage::S3 => false,
        NativeLineage::S3Ls => s3::ls::DESTINATIONS.contains(&dest),
        NativeLineage::S3Cp => s3::cp_args::DESTINATIONS.contains(&dest),
        NativeLineage::S3Mv => s3::mv::DESTINATIONS.contains(&dest),
        NativeLineage::S3Rm => s3::rm::DESTINATIONS.contains(&dest),
        NativeLineage::S3Sync => s3::sync::DESTINATIONS.contains(&dest),
    }
}
#[cfg(test)]
mod tests;

fn unparsed(file: &ConfigFile) -> bool {
    matches!(file.status, Status::ParseFailure(_) | Status::ResourceLimit)
}
fn session_fallback(files: &ParsedFiles) -> Option<PythonReason> {
    if unparsed(&files.config) || unparsed(&files.credentials) {
        return Some(PythonReason::ConfigParseFailure);
    }
    has_plugins_section(files).then_some(PythonReason::Plugins)
}
fn has_plugins_section(files: &ParsedFiles) -> bool {
    files.organized.ordinary.contains_key("plugins")
}
/// [`unparsed`], plus a directory at the alias path. AliasLoader reads
/// whatever exists there (`AliasLoader._load_aliases`) and raw_config_parse
/// refuses a path that is not a regular file with `ConfigNotFound`; the
/// session catches that for the shared files (`Session.full_config`),
/// CLIDriver.main does not.
fn unusable_alias_file(file: &ConfigFile) -> bool {
    unparsed(file) || file.status == Status::Directory
}
/// Whether the alias file replaces or extends the grammar of `root`.
/// AliasLoader reads the file with `raw_config_parse(parse_subsections=False)`,
/// a RawConfigParser whose section names are kept literally and whose
/// option names are lowercased; awscli-config stores them the same way,
/// with DEFAULT entries inherited into every section as configparser does.
/// A `toplevel` section (exact name) replaces commands by name
/// (`AliasCommandInjector`), so its option names, lowercased, are compared
/// with the root as given; a section whose name starts with `command ` is
/// split on whitespace into a lineage (`AliasLoader._normalize_key_names`)
/// whose entries inject into that command's table when it is built
/// (`AliasSubCommandInjector`).
fn defines_alias(files: &ParsedFiles, root: &str) -> bool {
    files.aliases.status == Status::Loaded
        && files.aliases.sections.iter().any(|(name, entries)| {
            (name == "toplevel" && entries.get(root).is_some())
                || (!entries.is_empty()
                    && name.starts_with("command ")
                    && python_split(name).nth(1) == Some(root))
        })
}
/// Python's `str.split()`: the words between runs of `str.isspace()`
/// characters, whose set includes U+001C to U+001F.
fn python_split(text: &str) -> impl Iterator<Item = &str> {
    text.split(awscli_config::is_space)
        .filter(|word| !word.is_empty())
}
