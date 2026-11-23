//! An admitted model operation, run as `CLIOperationCaller.invoke` runs
//! it: the client the session would create, the call, and the response
//! printed by the formatter `--output` selects, to stdout or the pager.
//!
//! [`Invocation::preflight`] does everything that needs neither a native
//! runtime nor the network: the output format, the pager, the timestamp
//! parser, the credential chain, the client, and the client's own refusals
//! ([`Client::prepare`]), any of which can still hand the invocation to
//! Python. [`Invocation::run`] then sends the call.
use crate::admit::Admitted;
use crate::failure;
use cli_core::cli_error::{CommandFailure, PythonReason};
use cli_core::client_config::{ClientSettings, Trust};
use cli_core::{EnvMap, aws_profile, shared_config};
use client::call::{Client, ClientParts, UserAgentContext};
use client::credentials::ChainCredentials;
use client::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use client::error::{CallError, ParseError};
use client::hooks::Customization;
use client::http::{HttpRequest, HttpResponse};
use client::protocol::ParseOptions;
use client::transport::{Timeouts, Transport, TransportError};
use command::{Context, Invocation, Preflight};
use format::{Delivered, Destination, Outcome, OutputFormat, TimestampFormat, TimestampParser};
use model::shapes::Model;
use model::value::Value;
use std::cell::OnceCell;
use std::ffi::OsString;
use std::sync::{Arc, Mutex, OnceLock};

/// What binding decided, and what the invocation needs to build its client.
#[derive(Debug)]
pub struct ServiceInvocation {
    pub admitted: Admitted,
    pub model: Arc<Model>,
    pub endpoints: EndpointData,
    pub customizations: &'static [&'static Customization],
    /// The hyphenized service id the configured-endpoint lookup keys on.
    pub service_id: String,
    /// `--output`, when given.
    pub output: Option<String>,
    pub no_cli_pager: bool,
    /// The output shape reaches a timestamp member.
    pub output_timestamps: bool,
    /// The payload's CA bundle, trusted when nothing configures another.
    pub embedded_trust: Option<&'static [u8]>,
    pub versions: cli_core::client_config::user_agent::Versions<'static>,
    ready: OnceCell<Ready>,
}

/// What the preflight settled.
#[derive(Debug)]
struct Ready {
    /// The client, or the failure botocore raises as it creates one or as
    /// it prepares the call, which the run reports.
    client: Result<Client, CallError>,
    format: OutputFormat,
    destination: Destination,
    pager_env: Vec<(OsString, OsString)>,
}

impl ServiceInvocation {
    #[expect(clippy::too_many_arguments, reason = "one field each, from binding")]
    pub fn new(
        admitted: Admitted,
        model: Arc<Model>,
        endpoints: EndpointData,
        customizations: &'static [&'static Customization],
        service_id: String,
        output: Option<String>,
        no_cli_pager: bool,
        embedded_trust: Option<&'static [u8]>,
        versions: cli_core::client_config::user_agent::Versions<'static>,
    ) -> Self {
        let output_timestamps = model
            .operation(&admitted.operation)
            .and_then(|operation| operation.output)
            .is_some_and(|shape| crate::admit::reaches_timestamp(&model, shape));
        Self {
            admitted,
            model,
            endpoints,
            customizations,
            service_id,
            output,
            no_cli_pager,
            output_timestamps,
            embedded_trust,
            versions,
            ready: OnceCell::new(),
        }
    }
}

/// The session's `timestamp_parser`, set once by the preflight; the
/// client's parse options are `'static` functions that read it.
static TIMESTAMPS: OnceLock<TimestampParser> = OnceLock::new();

fn parse_timestamp(wire: &Value) -> Result<Value, ParseError> {
    let parser = TIMESTAMPS
        .get()
        .copied()
        .unwrap_or_else(|| TimestampParser::new(TimestampFormat::Iso8601, Some(0)));
    parser
        .parse(wire)
        .map_err(|error| ParseError::new(format!("{error:?}")))
}

/// The CLI's identity blob parser (`register_identity_blob_parser`): a blob
/// prints as the base64 text it arrived as.
fn identity_blob(text: &str) -> Result<Value, ParseError> {
    Ok(Value::String(text.to_owned()))
}

const CLI_PARSE: ParseOptions<'static> = ParseOptions {
    timestamp: &parse_timestamp,
    blob: &identity_blob,
};

impl Invocation for ServiceInvocation {
    fn preflight(&self, cx: &Preflight<'_>) -> Result<(), PythonReason> {
        let format = output_format(cx, self.output.as_deref());
        let parser = timestamp_parser(cx)?;
        if parser.needs_local_zone() && self.output_timestamps {
            return Err(PythonReason::UnsupportedTrait);
        }
        let destination = destination(cx, self.no_cli_pager)?;
        if !awscli_argparse::unpack::text_encoding_is_utf8(cx.variables)
            || (utf8_mode(cx.env)
                && format == OutputFormat::Json
                && destination != Destination::Stdout)
        {
            return Err(PythonReason::OutputEncoding);
        }
        let _ = TIMESTAMPS.set(parser);
        let client = self.client(cx)?;
        let client = match client {
            Ok(client) => {
                match client.prepare(&self.admitted.operation, self.admitted.input.clone()) {
                    Err(CallError::Unsupported(_)) => return Err(PythonReason::UnsupportedTrait),
                    Err(error) => Err(error),
                    Ok(_) => Ok(client),
                }
            }
            Err(CallError::Unsupported(_)) => return Err(PythonReason::UnsupportedProtocol),
            Err(error) => Err(error),
        };
        let pager_env = format::pager::environment(
            cx.variables
                .iter()
                .map(|(name, value)| (OsString::from(name), value.clone())),
        );
        let _ = self.ready.set(Ready {
            client,
            format,
            destination,
            pager_env,
        });
        Ok(())
    }

    fn run(&self, cx: &mut Context<'_>) -> Result<u8, CommandFailure> {
        let Some(ready) = self.ready.get() else {
            return Err(CommandFailure {
                message: b"the invocation ran without its preflight".to_vec(),
                ..CommandFailure::new(cli_core::cli_error::Category::General)
            });
        };
        let client = ready
            .client
            .as_ref()
            .map_err(|error| failure::call_failure(error, &self.model))?;
        let output = match client.call(&self.admitted.operation, self.admitted.input.clone()) {
            Ok(output) => output,
            Err(CallError::Interrupted) => return Ok(interrupted(cx)),
            Err(error) => return Err(failure::call_failure(&error, &self.model)),
        };
        let text = match format::render(&output.value, ready.format) {
            Ok(text) => text,
            Err(partial) => {
                let _ = cx.stdout.write_all(partial.written.as_bytes());
                return Err(CommandFailure {
                    message: partial.error.message.into_bytes(),
                    ..CommandFailure::new(cli_core::cli_error::Category::General)
                });
            }
        };
        let interrupt = cli_core::interrupt::process_state();
        match format::deliver(
            &ready.destination,
            &ready.pager_env,
            &text,
            cx.stdout,
            interrupt,
        ) {
            Ok(Delivered::Paged(Outcome::Interrupted)) => Ok(interrupted(cx)),
            // JSONFormatter swallows a failed write; the exit-time flush then
            // reports the closed stream as the s3 commands' does.
            Ok(Delivered::Stdout(Err(_))) if ready.format.swallows_write_errors() => Ok(0),
            Ok(Delivered::Stdout(Err(error))) => Err(CommandFailure {
                message: error.to_string().into_bytes(),
                ..CommandFailure::new(cli_core::cli_error::Category::General)
            }),
            Ok(Delivered::Stdout(Ok(())) | Delivered::Paged(Outcome::Exited)) => Ok(0),
            Err(error) => Err(failure::pager_failure(&error)),
        }
    }
}

/// `InterruptExceptionHandler`'s status, its newline written if stdout
/// takes it.
fn interrupted(cx: &mut Context<'_>) -> u8 {
    failure::interrupted(cx.stdout).unwrap_or(cli_core::interrupt::EXIT_STATUS)
}

impl ServiceInvocation {
    /// The client the session would create for this invocation, or the
    /// failure its creation raises; a credential source the native chain
    /// does not fetch from is Python's.
    fn client(&self, cx: &Preflight<'_>) -> Result<Result<Client, CallError>, PythonReason> {
        let globals = cx.globals;
        let profile = globals.profile.as_deref();
        let region = cli_core::region::configured_region(
            cx.env,
            cx.shared_files,
            globals.region.as_deref(),
            profile,
        )
        .map_err(|_| PythonReason::EndpointSetting)?;
        let configured = cli_core::endpoint_config::resolve(
            cx.env,
            cx.shared_files,
            &self.service_id,
            globals.endpoint_url.as_deref(),
            profile,
        )
        .map_err(|_| PythonReason::EndpointSetting)?;
        let credentials = credentials(cx)?;
        let settings = cx.settings;
        let endpoint = EndpointConfig::new(&EndpointInputs {
            region,
            use_fips: settings
                .use_fips_endpoint
                .as_ref()
                .map(|setting| setting.value),
            use_dual_stack: settings
                .use_dualstack_endpoint
                .as_ref()
                .map(|setting| setting.value),
            endpoint_url: configured.endpoint.as_deref(),
            signing: settings.signing.value,
        });
        let transport = Arc::new(LazyTransport::new(
            trust(settings, self.embedded_trust),
            settings.keep_alive.value,
            cli_core::SnapshotEnv::capture(),
            globals.debug,
        ));
        Ok(Client::new(ClientParts {
            service: self.admitted.service.clone(),
            model: Arc::clone(&self.model),
            endpoints: self.endpoints.clone(),
            endpoint,
            settings: settings.clone(),
            credentials: Arc::new(credentials),
            transport,
            runtime: Arc::new(client::runtime::SystemRuntime::new(
                cli_core::interrupt::process_state(),
            )),
            customizations: self.customizations,
            parse: CLI_PARSE,
            user_agent: UserAgentContext {
                host: cli_core::client_config::user_agent::host(),
                distribution: cli_core::client_config::user_agent::distribution(),
                versions: self.versions,
            },
        }))
    }
}

/// botocore's chain for the selected profile, walked before the client
/// exists, as `Session.create_client` walks it. A chain that would ask the
/// container's endpoint or the instance metadata service is Python's: the
/// model client does not fetch from them.
fn credentials(cx: &Preflight<'_>) -> Result<ChainCredentials, PythonReason> {
    if !cx.settings.signing.value {
        return Ok(ChainCredentials::new(None));
    }
    let selected = cx.globals.profile.as_deref();
    let name = aws_profile::select_or_default(cx.env, selected)
        .to_str()
        .unwrap_or("default");
    let profile = if aws_profile::is_explicit(selected) {
        ::credentials::Profile::Explicit(name)
    } else {
        ::credentials::Profile::Ambient(name)
    };
    let env = ::credentials::environment(cx.env);
    let asked = std::cell::Cell::new(false);
    let container = ::credentials::ContainerFetch::new(|_| {
        asked.set(true);
        Ok(None)
    });
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let noted = Arc::clone(&reached);
    let instance = ::credentials::RoleFetcher::new(move || {
        noted.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(None)
    });
    let found =
        ::credentials::default_resolver(&env, cx.shared_files, profile, container, instance)
            .load_credentials()
            .map_err(|_| PythonReason::CredentialForm)?;
    if found.is_none() && (asked.get() || reached.load(std::sync::atomic::Ordering::Relaxed)) {
        return Err(PythonReason::CredentialForm);
    }
    Ok(ChainCredentials::new(found))
}

/// The `--output` format: `--output`, else `AWS_DEFAULT_OUTPUT`, else the
/// profile's `output`, else `json` (`_construct_cli_output_chain`). The
/// router refused any other.
fn output_format(cx: &Preflight<'_>, output: Option<&str>) -> OutputFormat {
    let profile = aws_profile::select_or_default(cx.env, cx.globals.profile.as_deref());
    let variable = cx.env.var_os("AWS_DEFAULT_OUTPUT");
    let name = output
        .or_else(|| variable.and_then(std::ffi::OsStr::to_str))
        .or_else(|| shared_config::profile_property(cx.shared_files, profile, "output"))
        .unwrap_or("json");
    OutputFormat::from_name(name).unwrap_or(OutputFormat::Json)
}

/// `add_timestamp_parser`: the profile's `cli_timestamp_format` and this
/// process's local zone.
fn timestamp_parser(cx: &Preflight<'_>) -> Result<TimestampParser, PythonReason> {
    let profile = aws_profile::select_or_default(cx.env, cx.globals.profile.as_deref());
    let setting = shared_config::profile_property(cx.shared_files, profile, "cli_timestamp_format");
    let format = TimestampFormat::from_setting(setting).map_err(|_| PythonReason::OutputFormat)?;
    let offset = cli_core::local_zone::LocalZone::system().and_then(|zone| zone.fixed_offset());
    Ok(TimestampParser::new(format, offset))
}

/// `OutputStreamFactory`'s choice of stdout or the pager.
fn destination(cx: &Preflight<'_>, no_cli_pager: bool) -> Result<Destination, PythonReason> {
    let profile = aws_profile::select_or_default(cx.env, cx.globals.profile.as_deref());
    let cli_pager =
        if shared_config::profile_property_is_block(cx.shared_files, profile, "cli_pager") {
            format::Profile::Block
        } else {
            shared_config::profile_property(cx.shared_files, profile, "cli_pager")
                .map_or(format::Profile::Absent, format::Profile::Text)
        };
    let aws_pager = cx.env.var_os("AWS_PAGER");
    let pager = cx.env.var_os("PAGER");
    let chain = format::Chain {
        no_cli_pager,
        aws_pager,
        cli_pager,
        pager,
    };
    format::pager::destination(&chain, cx.stdout_is_tty).map_err(|_| PythonReason::Pager)
}

/// Python's UTF-8 mode, which `PYTHONUTF8=1` turns on.
fn utf8_mode(env: &cli_core::SnapshotEnv) -> bool {
    env.var_os("PYTHONUTF8").is_some_and(|value| value == "1")
}

/// What the client trusts: a configured bundle, else the payload's, else
/// the platform's.
fn trust(settings: &ClientSettings, embedded: Option<&'static [u8]>) -> ::crt::runtime::Trust {
    match &settings.trust.value {
        Trust::File(path) => ::crt::runtime::Trust::File {
            path: path.clone(),
            setting: settings.trust.source.setting_name(),
        },
        Trust::Embedded => embedded.map_or(::crt::runtime::Trust::Platform, |pem| {
            ::crt::runtime::Trust::Pem(pem.to_vec())
        }),
        Trust::Platform => ::crt::runtime::Trust::Platform,
    }
}

/// The CRT transport, built when the first request is sent: no native
/// runtime starts before the preflight is done, so a Python fallback finds
/// the process as the router left it.
#[derive(Debug)]
struct LazyTransport {
    inputs: Mutex<Option<(::crt::runtime::Trust, bool, cli_core::SnapshotEnv, bool)>>,
    built: OnceLock<Result<client::transport::crt::CrtTransport, TransportError>>,
}

impl LazyTransport {
    fn new(
        trust: ::crt::runtime::Trust,
        keep_alive: bool,
        env: cli_core::SnapshotEnv,
        debug: bool,
    ) -> Self {
        Self {
            inputs: Mutex::new(Some((trust, keep_alive, env, debug))),
            built: OnceLock::new(),
        }
    }
}

impl Transport for LazyTransport {
    fn send(
        &self,
        request: &HttpRequest,
        timeouts: Timeouts,
    ) -> Result<HttpResponse, TransportError> {
        let built = self.built.get_or_init(|| {
            let inputs = self.inputs.lock().ok().and_then(|mut inputs| inputs.take());
            let Some((trust, keep_alive, env, debug)) = inputs else {
                return Err(TransportError::Client {
                    reason: "the transport's inputs were taken".into(),
                });
            };
            client::transport::crt::CrtTransport::new(
                trust,
                keep_alive,
                env,
                ::crt::debug::Logger::new(debug),
            )
        });
        match built {
            Ok(transport) => transport.send(request, timeouts),
            Err(error) => Err(error.clone()),
        }
    }
}

#[cfg(test)]
mod tests;
