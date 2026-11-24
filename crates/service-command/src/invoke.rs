//! An admitted model operation, run as `CLIOperationCaller.invoke` runs
//! it: the client the session would create, the call, and the response
//! printed by the formatter `--output` selects, to stdout or the pager.
//!
//! [`Invocation::preflight`] does everything that needs neither a native
//! runtime nor the network: the output format, the pager, the timestamp
//! parser, the credential chain, the client, and the client's own refusals
//! ([`Client::prepare`]), any of which can still hand the invocation to
//! Python. [`Invocation::run`] then sends the call, or pages through the
//! paginator's calls ([`paged`]).
use crate::admit::Admitted;
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
    /// The output shape reaches a timestamp the wire sends as an epoch.
    pub output_epochs: bool,
    /// The payload's CA bundle, trusted when nothing configures another.
    pub embedded_trust: Option<&'static [u8]>,
    pub versions: cli_core::client_config::user_agent::Versions<'static>,
    ready: OnceCell<Ready>,
}

/// What the preflight settled.
#[derive(Debug)]
struct Ready {
    /// The client, or the failure botocore raises as its session loads
    /// credentials, creates the client or prepares the call, which the run
    /// reports.
    client: Result<Client, CommandFailure>,
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
        let output_epochs = model
            .operation(&admitted.operation)
            .and_then(|operation| operation.output)
            .is_some_and(|shape| crate::admit::reaches_epoch_timestamp(&model, shape));
        Self {
            admitted,
            model,
            endpoints,
            customizations,
            service_id,
            output,
            no_cli_pager,
            output_epochs,
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
        if parser.needs_local_zone() && self.output_epochs {
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
        let failure = |error: CallError| service_errors::call_failure(&error, &self.model);
        let client = match self.client(cx)? {
            Ok(Ok(client)) => {
                let prepared = match self.first_request(&client) {
                    Ok(Some(input)) => {
                        Some(client.prepare(&self.admitted.operation, input).map(drop))
                    }
                    Ok(None) => None,
                    Err(unsupported) => Some(Err(unsupported)),
                };
                match prepared {
                    Some(Err(CallError::Unsupported(_))) => {
                        return Err(PythonReason::UnsupportedTrait);
                    }
                    Some(Err(error)) => Err(failure(error)),
                    Some(Ok(())) | None => Ok(client),
                }
            }
            Ok(Err(CallError::Unsupported(_))) => return Err(PythonReason::UnsupportedProtocol),
            Ok(Err(error)) => Err(failure(error)),
            Err(credentials) => Err(credentials),
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
            return Err(general(
                b"the invocation ran without its preflight".to_vec(),
            ));
        };
        if let Some(raised) = &self.admitted.raised {
            return Err(raised.clone());
        }
        if self.admitted.warned {
            // uni_print writes to sys.stderr, which Python writes at once.
            let warning = crate::bind::MAX_ITEMS_WARNING.as_bytes();
            let _ = cx
                .stderr
                .write_all(warning)
                .and_then(|()| cx.stderr.flush());
        }
        let client = ready.client.as_ref().map_err(Clone::clone)?;
        if let Some(paginated) = &self.admitted.paginated {
            return paged::run(self, cx, ready, client, paginated);
        }
        match client.call(&self.admitted.operation, self.admitted.input.clone()) {
            Ok(output) => self.print(cx, ready, &output.value),
            Err(error) => self.call_failed(cx, error),
        }
    }
}

impl ServiceInvocation {
    /// The input of the first request: the call's, or the paginator's first
    /// page's (`_inject_starting_params`). `None` when the paginator fails
    /// before that request, which the run's first page reports, and
    /// [`CallError::Unsupported`] for a starting token only the Python
    /// route reads.
    fn first_request(&self, client: &Client) -> Result<Option<Value>, CallError> {
        let Some(paginated) = &self.admitted.paginated else {
            return Ok(Some(self.admitted.input.clone()));
        };
        let first = client::paginate::PageIterator::new(
            client,
            &self.admitted.operation,
            self.admitted.input.clone(),
            paginated.config.clone(),
            paginated.pagination.clone(),
        )
        .first_input();
        match first {
            Ok(input) => Ok(Some(input)),
            Err(unsupported @ CallError::Unsupported(_)) => Err(unsupported),
            Err(_) => Ok(None),
        }
    }

    /// What the formatter writes for `response`, delivered where
    /// `OutputStreamFactory` sends it.
    fn print(
        &self,
        cx: &mut Context<'_>,
        ready: &Ready,
        response: &Value,
    ) -> Result<u8, CommandFailure> {
        let text = match format::render(response, ready.format) {
            Ok(text) => text,
            Err(partial) => {
                // TextFormatter flushes what it wrote before raising.
                let _ = cx
                    .stdout
                    .write_all(partial.written.as_bytes())
                    .and_then(|()| cx.stdout.flush());
                return Err(general(partial.error.message.into_bytes()));
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
            Ok(Delivered::Stdout(Err(error))) => Err(general(error.to_string().into_bytes())),
            Ok(Delivered::Stdout(Ok(())) | Delivered::Paged(Outcome::Exited)) => Ok(0),
            Err(error) => Err(service_errors::pager_failure(&error)),
        }
    }

    /// What a failed call ends with: `KeyboardInterrupt`'s status, or the
    /// failure botocore's exception is.
    fn call_failed(&self, cx: &mut Context<'_>, error: CallError) -> Result<u8, CommandFailure> {
        match error {
            CallError::Interrupted => Ok(interrupted(cx)),
            error => Err(service_errors::call_failure(&error, &self.model)),
        }
    }
}

/// `GeneralExceptionHandler`'s failure with `str()` of the exception.
fn general(message: Vec<u8>) -> CommandFailure {
    CommandFailure {
        message,
        ..CommandFailure::new(cli_core::cli_error::Category::General)
    }
}

/// `InterruptExceptionHandler`'s status, its newline written if stdout
/// takes it.
fn interrupted(cx: &mut Context<'_>) -> u8 {
    service_errors::interrupted(cx.stdout).unwrap_or(cli_core::interrupt::EXIT_STATUS)
}

impl ServiceInvocation {
    /// The client the session would create for this invocation, or the
    /// failure its credentials (outside) or its creation (inside) raise; a
    /// credential source the native chain does not fetch from is Python's.
    fn client(
        &self,
        cx: &Preflight<'_>,
    ) -> Result<Result<Result<Client, CallError>, CommandFailure>, PythonReason> {
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
        let credentials = match credentials(cx)? {
            Ok(credentials) => credentials,
            Err(failure) => return Ok(Err(failure)),
        };
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
        Ok(Ok(Client::new(ClientParts {
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
        })))
    }
}

/// botocore's chain for the selected profile, walked before the client
/// exists, as `Session.create_client` walks it. A chain that would ask the
/// container's endpoint or the instance metadata service is Python's: the
/// model client does not fetch from them. A provider's failure is
/// botocore's, which the run reports.
fn credentials(
    cx: &Preflight<'_>,
) -> Result<Result<ChainCredentials, CommandFailure>, PythonReason> {
    if !cx.settings.signing.value {
        return Ok(Ok(ChainCredentials::new(None)));
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
            .load_credentials();
    let found = match found {
        Ok(found) => found,
        Err(error) => return Ok(Err(service_errors::credentials_failure(&error))),
    };
    if found.is_none() && (asked.get() || reached.load(std::sync::atomic::Ordering::Relaxed)) {
        return Err(PythonReason::CredentialForm);
    }
    Ok(Ok(ChainCredentials::new(found)))
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
fn trust(
    settings: &ClientSettings,
    embedded: Option<&'static [u8]>,
) -> client::transport::crt::Trust {
    match &settings.trust.value {
        Trust::File(path) => client::transport::crt::Trust::File {
            path: path.clone(),
            setting: settings.trust.source.setting_name(),
        },
        Trust::Embedded => embedded.map_or(client::transport::crt::Trust::Platform, |pem| {
            client::transport::crt::Trust::Pem(pem.to_vec())
        }),
        Trust::Platform => client::transport::crt::Trust::Platform,
    }
}

/// The CRT transport, built when the first request is sent: no native
/// runtime starts before the preflight is done, so a Python fallback finds
/// the process as the router left it.
#[derive(Debug)]
struct LazyTransport {
    inputs: Mutex<
        Option<(
            client::transport::crt::Trust,
            bool,
            cli_core::SnapshotEnv,
            bool,
        )>,
    >,
    built: OnceLock<Result<client::transport::crt::CrtTransport, TransportError>>,
}

impl LazyTransport {
    fn new(
        trust: client::transport::crt::Trust,
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
        // A Ctrl-C recorded while a paginated call fetches its pages: no
        // further request starts, as KeyboardInterrupt ends Python's
        // iteration (`paged`).
        if cli_core::interrupt::process_state().is_requested() {
            return Err(TransportError::Interrupted);
        }
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
                client::transport::crt::Logger::new(debug),
            )
        });
        match built {
            Ok(transport) => transport.send(request, timeouts),
            Err(error) => Err(error.clone()),
        }
    }
}

mod paged;

#[cfg(test)]
mod tests;
