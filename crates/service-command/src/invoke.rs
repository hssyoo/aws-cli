//! An admitted model operation, run as `CLIOperationCaller.invoke` runs
//! it: the client the session would create, the call, and the response
//! printed by the formatter `--output` selects, to stdout or the pager.
//!
//! [`Invocation::preflight`] does everything that needs neither a native
//! runtime, the network nor a credential provider: the output format, the
//! pager, the timestamp parser, whether the credential chain would reach a
//! service the model client does not fetch from, the client, and the
//! client's own refusals ([`Client::prepare`]), any of which can still hand
//! the invocation to Python. [`Invocation::run`] then sends the prepared
//! call, and the credential chain is walked then, once
//! (`crate::credentials`). A failure the preflight settled is reported by
//! the run after the chain is walked, as botocore walks it in
//! `create_client` before it validates or resolves anything: a failing
//! chain's error is the one reported. A paginated call pages through the
//! paginator's calls ([`paged`]) with the same client and chain.
use crate::admit::Admitted;
use cli_core::cli_error::{CommandFailure, PythonReason};
use cli_core::client_config::{ClientSettings, Trust};
use cli_core::{EnvMap, aws_profile};
use client::call::Prepared;
use client::call::{Client, ClientParts, UserAgentContext};
use client::credentials::{ChainCredentials, CredentialSource};
use client::endpoint::{EndpointConfig, EndpointInputs};
use client::error::{CallError, ParseError};
use client::hooks::Customization;
use client::http::{HttpRequest, HttpResponse};
use client::paginate::PageIterator;
use client::transport::{Timeouts, Transport, TransportError};
use command::{Context, Invocation, Preflight};
use format::{Delivered, Destination, Outcome, OutputFormat, TimestampParser};
use model::shapes::Model;
use model::value::Value;
use std::cell::{OnceCell, RefCell};
use std::ffi::OsString;
use std::sync::{Arc, Mutex, OnceLock};

/// What binding decided, and what the invocation needs to build its client.
#[derive(Debug)]
pub struct ServiceInvocation {
    pub admitted: Admitted,
    pub model: Arc<Model>,
    pub customizations: &'static [&'static Customization],
    /// The hyphenized service id the configured-endpoint lookup keys on.
    pub service_id: String,
    /// The output shape reaches a timestamp the wire sends as an epoch.
    pub output_epochs: bool,
    /// The payload's CA bundle, trusted when nothing configures another.
    pub embedded_trust: Option<&'static [u8]>,
    pub versions: cli_core::client_config::user_agent::Versions<'static>,
    /// `--query`, which the formatter searches the response with.
    pub query: Option<crate::query::Query>,
    ready: OnceCell<Ready>,
}

/// What the preflight settled.
#[derive(Debug)]
struct Ready {
    /// The client and its call, or the failure botocore raises as it
    /// creates the client or prepares the call, which the run reports;
    /// taken by the one run.
    call: RefCell<Option<Result<Call, CommandFailure>>>,
    /// The client's credentials, which the run walks before it reports a
    /// failure.
    credentials: Arc<dyn CredentialSource>,
    format: OutputFormat,
    destination: Destination,
    /// Stdout is a terminal, on which CPython's `sys.stdout` is
    /// line-buffered.
    line_buffered: bool,
    pager_env: Vec<(OsString, OsString)>,
}

/// What the preflight prepared, once, which the run sends.
#[derive(Debug)]
enum Call {
    /// One request.
    Single { client: Client, prepared: Prepared },
    /// The paginator, its first request prepared, which owns the client.
    Paged(PageIterator<Client>),
}

impl ServiceInvocation {
    pub fn new(
        admitted: Admitted,
        model: Arc<Model>,
        customizations: &'static [&'static Customization],
        service_id: String,
        embedded_trust: Option<&'static [u8]>,
        versions: cli_core::client_config::user_agent::Versions<'static>,
    ) -> Self {
        let output_epochs = (model.schema().service())
            .operation(&admitted.operation)
            .and_then(|operation| operation.output())
            .is_some_and(|shape| crate::admit::reaches_epoch_timestamp(&model, shape));
        Self {
            admitted,
            model,
            customizations,
            service_id,
            output_epochs,
            embedded_trust,
            versions,
            query: None,
            ready: OnceCell::new(),
        }
    }

    /// This invocation, its response searched with `query`.
    #[must_use]
    pub fn with_query(self, query: Option<crate::query::Query>) -> Self {
        Self { query, ..self }
    }

    /// `--query`, as the formatter takes it.
    fn query(&self) -> Option<&dyn format::Query> {
        self.query.as_ref().map(|query| query as &dyn format::Query)
    }
}

impl Invocation for ServiceInvocation {
    fn preflight(&self, cx: &Preflight<'_>) -> Result<(), PythonReason> {
        let format = cx.output.format;
        let parser = cx.output.timestamps;
        if parser.needs_local_zone() && self.output_epochs {
            return Err(PythonReason::LocalZone);
        }
        let destination = format::pager::destination(&cx.output.pager.chain(), cx.stdout_is_tty)
            .map_err(|_| PythonReason::Pager)?;
        if !awscli_argparse::unpack::text_encoding_is_utf8(cx.variables)
            || (utf8_mode(cx.env)
                && format == OutputFormat::Json
                && destination != Destination::Stdout)
        {
            return Err(PythonReason::OutputEncoding);
        }
        let failure = |error: CallError| match error {
            CallError::Unsupported(unsupported) => Err(crate::admit::reason(&unsupported)),
            other => Ok(service_errors::call_failure(&other, &self.model)),
        };
        let input = self.input(cx)?;
        let credentials = credentials(cx)?;
        let call = match self.client(cx, Arc::clone(&credentials))? {
            Ok(client) => match self.prepare(client, input) {
                Ok(call) => Ok(call),
                Err(error) => Err(failure(error)?),
            },
            Err(error) => Err(failure(error)?),
        };
        let pager_env = format::pager::environment(
            cx.variables
                .iter()
                .map(|(name, value)| (OsString::from(name), value.clone())),
        );
        let _ = self.ready.set(Ready {
            call: RefCell::new(Some(call)),
            credentials,
            format,
            destination,
            line_buffered: cx.stdout_is_tty,
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
        // `--max-items` warns as the call's parameters are built, before
        // botocore creates the client.
        if self.admitted.warned {
            // uni_print writes to sys.stderr, which Python writes at once.
            let warning = crate::bind::MAX_ITEMS_WARNING.as_bytes();
            let _ = cx
                .stderr
                .write_all(warning)
                .and_then(|()| cx.stderr.flush());
        }
        let taken = ready.call.borrow_mut().take();
        let (client, prepared) = match taken {
            Some(Ok(Call::Single { client, prepared })) => (client, prepared),
            Some(Ok(Call::Paged(pages))) => return paged::run(self, cx, ready, pages),
            Some(Err(failure)) => return Err(self.after_the_chain(&*ready.credentials, &failure)),
            None => return Err(general(b"the prepared call was already sent".to_vec())),
        };
        // KeyboardInterrupt ends the call wherever it lands: the call is
        // active work, so a Ctrl-C is recorded rather than ending the
        // process, and the call ends interrupted once its request in flight,
        // which the transport cannot cut short, returns (`paged` does the
        // same for each page).
        let interrupt = cli_core::interrupt::process_state();
        let mut scope = cli_core::interrupt::Scope::enter(interrupt);
        let called = client.send(prepared);
        let called = if interrupt.is_requested() {
            Err(CallError::Interrupted)
        } else {
            called
        };
        scope.leave();
        match called {
            Ok(output) => {
                let value = self.save_file(cx, output.value)?;
                self.print(cx, ready, value)
            }
            Err(error) => self.call_failed(cx, error),
        }
    }
}

/// The most of a streaming blob member's file the native route reads into
/// its request, which holds the body whole where botocore streams it from
/// the open file; a larger file is Python's.
pub const INPUT_FILE_BYTES_MAX: u64 = 64 * 1024 * 1024;

impl ServiceInvocation {
    /// The admitted input with each streaming blob member's file read in,
    /// as `unpack_scalar_cli_arg` opens it and botocore sends it: the path
    /// through `os.path.expandvars`, then `expanduser`, relative to the
    /// working directory. A path that names no regular file raises
    /// `ParamError` in Python, one it cannot open an `OSError`; both, and
    /// a path native does not expand or a file beyond
    /// [`INPUT_FILE_BYTES_MAX`], are Python's to run.
    fn input(&self, cx: &Preflight<'_>) -> Result<Value, PythonReason> {
        let mut input = self.admitted.input.clone();
        if self.admitted.files.is_empty() {
            return Ok(input);
        }
        let Value::Map(members) = &mut input else {
            return Err(PythonReason::UnsupportedValue);
        };
        for file in &self.admitted.files {
            let bytes = read_input_file(&file.path, cx.variables, cx.directory)
                .ok_or(PythonReason::UnsupportedValue)?;
            members.insert(file.member.to_string(), Value::Blob(bytes));
        }
        Ok(input)
    }

    /// `StreamingOutputArgument.save_file` on `after-call`: the streamed
    /// member written to `outfile`, created or truncated, then removed
    /// from the response the formatter prints. An `OSError` opening or
    /// writing the file reaches the general handler, the response
    /// unprinted.
    fn save_file(&self, cx: &Context<'_>, response: Value) -> Result<Value, CommandFailure> {
        let Some(outfile) = &self.admitted.outfile else {
            return Ok(response);
        };
        let Value::Map(mut parsed) = response else {
            return Ok(response);
        };
        let Some(body) = parsed.remove(&outfile.response_key) else {
            return Ok(Value::Map(parsed));
        };
        let Value::Blob(bytes) = &body else {
            return Err(general(b"the streamed member is not bytes".to_vec()));
        };
        write_outfile(&outfile.path, cx.directory, bytes)?;
        Ok(Value::Map(parsed))
    }
}

/// The file `path` names, expanded as `unpack_scalar_cli_arg` expands it,
/// when it is a regular file native reads whole.
fn read_input_file(
    path: &str,
    variables: &std::collections::HashMap<String, OsString>,
    directory: &std::path::Path,
) -> Option<Vec<u8>> {
    use awscli_argparse::unpack::{expand_user, expand_vars};
    // Windows expands `%VAR%` and `USERPROFILE` too; Python runs it there.
    if !cfg!(unix) {
        return None;
    }
    let expanded = expand_user(&expand_vars(path.as_bytes(), variables)?, variables)?;
    let full = directory.join(String::from_utf8(expanded).ok()?);
    let metadata = std::fs::metadata(&full).ok()?;
    if !metadata.is_file() || metadata.len() > INPUT_FILE_BYTES_MAX {
        return None;
    }
    let mut bytes = Vec::new();
    let file = std::fs::File::open(&full).ok()?;
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, INPUT_FILE_BYTES_MAX + 1),
        &mut bytes,
    )
    .ok()?;
    (bytes.len() as u64 <= INPUT_FILE_BYTES_MAX).then_some(bytes)
}

/// `open(path, 'wb')` and its writes: `str()` of the `OSError` either
/// raises, the path named for the open's.
fn write_outfile(
    path: &str,
    directory: &std::path::Path,
    bytes: &[u8],
) -> Result<(), CommandFailure> {
    use std::io::Write as _;
    let named = |error: std::io::Error| {
        let text = os_error(&error);
        let repr = awscli_argparse::pyrepr::os(std::ffi::OsStr::new(path));
        general(format!("{text}: {repr}").into_bytes())
    };
    // `open('')` names no file, where joining it would name the directory.
    let full = if path.is_empty() {
        std::path::PathBuf::new()
    } else {
        directory.join(path)
    };
    let mut file = std::fs::File::create(full).map_err(named)?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|error| general(os_error(&error).into_bytes()))
}

/// `[Errno N] strerror` of an `OSError`, the text the C library gives the
/// error number, which `std::io::Error` writes before ` (os error N)`.
fn os_error(error: &std::io::Error) -> String {
    let Some(errno) = error.raw_os_error() else {
        return error.to_string();
    };
    let shown = error.to_string();
    let text = shown
        .strip_suffix(&format!(" (os error {errno})"))
        .unwrap_or(&shown);
    format!("[Errno {errno}] {text}")
}

impl ServiceInvocation {
    /// The call `client` makes, prepared: the request, or the paginator
    /// with its first request prepared (`_make_client_call`).
    ///
    /// # Errors
    /// What botocore raises before it sends, after it created the client:
    /// what `prepare` raises, and what the paginator raises before its
    /// first request (a bad starting token, a `MaxItems` or `PageSize` it
    /// refuses); [`CallError::Unsupported`] for a call, or a starting
    /// token, only the Python route takes.
    fn prepare(&self, client: Client, input: Value) -> Result<Call, CallError> {
        let admitted = &self.admitted;
        let Some(paginated) = &admitted.paginated else {
            let prepared = client.prepare(&admitted.operation, input)?;
            return Ok(Call::Single { client, prepared });
        };
        let mut pages = PageIterator::new(
            client,
            &admitted.operation,
            input,
            paginated.config.clone(),
            paginated.pagination.clone(),
        )?;
        pages.prepare()?;
        Ok(Call::Paged(pages))
    }

    /// What the formatter writes for `response`, delivered where
    /// `OutputStreamFactory` sends it.
    fn print(
        &self,
        cx: &mut Context<'_>,
        ready: &Ready,
        response: Value,
    ) -> Result<u8, CommandFailure> {
        let errors = ready.destination.errors();
        let text = match format::render_with(response, ready.format, errors, self.query()) {
            Ok(text) => text,
            Err(partial) => return self.partial(cx, ready, partial),
        };
        let interrupt = cli_core::interrupt::process_state();
        match format::deliver(
            &ready.destination,
            &ready.pager_env,
            &text,
            cx.stdout,
            ready.line_buffered,
            interrupt,
        ) {
            Ok(Delivered::Paged(Outcome::Interrupted)) => Ok(interrupted(cx)),
            // JSONFormatter swallows a failed write; the exit-time flush then
            // reports the closed stream as the s3 commands' does.
            Ok(Delivered::Stdout(Err(_))) if ready.format.swallows_write_errors() => Ok(0),
            Ok(Delivered::Stdout(Err(error))) => Err(write_failure(&error)),
            Ok(Delivered::Stdout(Ok(())) | Delivered::Paged(Outcome::Exited)) => Ok(0),
            Err(error) => Err(service_errors::pager_failure(&error)),
        }
    }

    /// What the formatter wrote before it raised, delivered where its stream
    /// went, then the failure: on stdout, flushed as `TextFormatter` flushes
    /// it; through the pager, which the first write started, whatever it
    /// wrote, and which is waited for before the failure is reported, as
    /// `get_pager_stream`'s `communicate()` waits.
    fn partial(
        &self,
        cx: &mut Context<'_>,
        ready: &Ready,
        partial: format::Partial,
    ) -> Result<u8, CommandFailure> {
        match &ready.destination {
            Destination::Stdout => {
                let _ = cx
                    .stdout
                    .write_all(&partial.written)
                    .and_then(|()| cx.stdout.flush());
            }
            Destination::Pager(argv) if partial.started => {
                let interrupt = cli_core::interrupt::process_state();
                match format::pager::run(argv, &ready.pager_env, &partial.written, interrupt) {
                    Ok(Outcome::Interrupted) => return Ok(interrupted(cx)),
                    Ok(Outcome::Exited) => {}
                    Err(error) => return Err(service_errors::pager_failure(&error)),
                }
            }
            Destination::Pager(_) => {}
        }
        Err(general(partial.error.message.into_bytes()))
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

/// The invocation of a parse an `operation-args-parsed` handler raised on
/// (`ensure_paging_params_not_set`): Python reports the failure before it
/// builds the request or creates a client, so this reports it and makes
/// neither.
#[derive(Debug)]
pub struct Raised(pub CommandFailure);

impl Invocation for Raised {
    fn run(&self, _cx: &mut Context<'_>) -> Result<u8, CommandFailure> {
        Err(self.0.clone())
    }
}

/// `GeneralExceptionHandler`'s failure for the `OSError` a write to stdout
/// raised, as `str()` of it reads.
fn write_failure(error: &std::io::Error) -> CommandFailure {
    general(cli_core::cli_error::stdout_write_failure_message(error).into_bytes())
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
    /// `failure`, which botocore raises after `create_client` has walked the
    /// credential chain: the chain's error instead, if it fails. The chain
    /// is the client's, so a provider still runs once.
    fn after_the_chain(
        &self,
        credentials: &dyn CredentialSource,
        failure: &CommandFailure,
    ) -> CommandFailure {
        match credentials.credentials() {
            Err(error) => {
                service_errors::call_failure(&CallError::Credentials(error.0), &self.model)
            }
            Ok(_) => failure.clone(),
        }
    }

    /// The client the session would create for this invocation, signing
    /// with `credentials`, or the failure its creation raises.
    fn client(
        &self,
        cx: &Preflight<'_>,
        credentials: Arc<dyn CredentialSource>,
    ) -> Result<Result<Client, CallError>, PythonReason> {
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
            cx.env.clone(),
            globals.debug,
        ));
        Ok(Client::new(ClientParts {
            service: self.admitted.service.clone(),
            model: Arc::clone(&self.model),
            endpoints: self.admitted.endpoints.clone(),
            endpoint,
            settings: settings.clone(),
            credentials,
            transport,
            runtime: Arc::new(client::runtime::SystemRuntime::new(
                cli_core::interrupt::process_state(),
            )),
            customizations: self.customizations,
            parse: cli_parse_hooks(cx.output.timestamps),
            user_agent: UserAgentContext {
                host: cli_core::client_config::user_agent::host(),
                distribution: cli_core::client_config::user_agent::distribution(),
                versions: self.versions,
                extras: session_extras(settings),
            },
        }))
    }
}

/// The CLI's response parse handlers, which the client owns: the session's
/// timestamp parser (`add_timestamp_parser`), and the identity blob parser
/// (`register_identity_blob_parser`), which leaves a blob as the base64 text
/// it arrived as.
fn cli_parse_hooks(timestamps: TimestampParser) -> client::protocol::ParseHooks {
    client::protocol::ParseHooks {
        timestamp: Arc::new(move |wire: &Value| {
            timestamps
                .parse(wire)
                .map_err(|error| ParseError::new(format!("{error:?}")))
        }),
        blob: Arc::new(|text: &Value| Ok(text.clone())),
    }
}

/// The CLI session's `User-Agent` extras, as the s3 commands send them:
/// the install source, the prompt mode, and the command's lineage.
fn session_extras(
    settings: &cli_core::client_config::ClientSettings,
) -> client::useragent::SessionExtras {
    use cli_core::client_config::user_agent::{INSTALLER, PROMPT};
    client::useragent::SessionExtras {
        installer: INSTALLER,
        prompt: PROMPT,
        command: settings.command.clone(),
    }
}

/// The client's credential source: botocore's chain for the selected
/// profile, walked when the client first signs (`crate::credentials`), or
/// none for an unsigned call. A chain that would ask the container's
/// endpoint or the instance metadata service is Python's. No credential
/// provider runs here.
fn credentials(cx: &Preflight<'_>) -> Result<Arc<dyn CredentialSource>, PythonReason> {
    if !cx.settings.signing.value {
        return Ok(Arc::new(ChainCredentials::new(None)));
    }
    let selected = cx.globals.profile.as_deref();
    let profile = crate::credentials::Selected {
        name: aws_profile::select_or_default(cx.env, selected)
            .to_str()
            .unwrap_or("default")
            .to_owned(),
        explicit: aws_profile::is_explicit(selected),
    };
    if crate::credentials::reaches_metadata(cx.env, cx.shared_files, &profile) {
        return Err(PythonReason::CredentialForm);
    }
    Ok(Arc::new(crate::credentials::LazyChain::new(
        cx.env,
        cx.shared_files,
        profile,
    )))
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
        // A Ctrl-C recorded while a call or a paginated call's pages are
        // fetched: no further request starts, as KeyboardInterrupt ends
        // Python's call or iteration (`run`, `paged`).
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
