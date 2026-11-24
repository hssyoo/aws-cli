//! The call loop: botocore's `BaseClient._make_api_call`,
//! `_convert_to_request_dict` and `Endpoint._send_request`, with the
//! interceptor stages at the points their botocore events fire.
//!
//! [`Client::prepare`] does everything that needs no network: the input
//! stages, the endpoint, validation, serialization and auth selection, so
//! a caller can send a call it cannot run natively to the Python route
//! before anything is sent. [`Client::send`] runs the attempts: sign, add
//! the unsigned `amz-sdk-invocation-id`, `amz-sdk-request` and
//! `Content-Length`, send, parse, and retry. botocore signs first:
//! `RequestSigner.handler` is registered on `request-created.<service>`,
//! which the emitter runs before `handlers.add_retry_headers` on the
//! generic `request-created` (hooks.py `PrefixTrie.prefix_search`: the more
//! specific event's handlers first), and `Content-Length` is set after the
//! event, when `Endpoint.create_request` prepares the request
//! (awsrequest.py `AWSRequestPreparer._prepare_headers`). So the signature
//! covers none of the three.
use crate::admission::Unsupported;
use crate::auth::{self, AuthScheme};
use crate::credentials::CredentialSource;
use crate::endpoint::{self, EndpointConfig, EndpointData, ResolvedEndpoint};
use crate::error::{CallError, ParseError, ServiceError};
use crate::hooks::{CallContext, Customization, HookError, Hooks, Slots};
use crate::http::{Headers, HttpRequest, HttpResponse, Method, Url};
use crate::protocol::{self, ParseHooks, Protocol, Serialized};
use crate::retry::{self, Attempt, ErrorKind, RetryDecision, Standard};
use crate::runtime::Runtime;
use crate::transport::{Timeouts, Transport, TransportError};
pub use crate::useragent::UserAgentContext;
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::Features;
use model::shapes::{Flag, Model, Operation};
use model::validate::{Validation, validate_input};
use model::value::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

/// botocore's standard mode default (`DEFAULT_MAX_ATTEMPTS`).
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Everything a client is built from, one per service.
#[derive(Debug)]
pub struct ClientParts {
    /// The model directory name, which customizations scope by.
    pub service: String,
    pub model: Arc<Model>,
    pub endpoints: EndpointData,
    pub endpoint: EndpointConfig,
    pub settings: ClientSettings,
    pub credentials: Arc<dyn CredentialSource>,
    pub transport: Arc<dyn Transport>,
    pub runtime: Arc<dyn Runtime>,
    pub customizations: &'static [&'static Customization],
    /// The response parsers' timestamp and blob handlers: the CLI's own,
    /// which it sets on every client (`add_timestamp_parser`).
    pub parse: ParseHooks,
    pub user_agent: UserAgentContext,
}

/// A call's output and what botocore's `ResponseMetadata` holds of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub value: Value,
    pub status: u16,
    pub headers: Headers,
    /// `RetryAttempts`: attempts after the first.
    pub retry_attempts: u32,
}

/// A client of one service.
#[derive(Debug)]
pub struct Client {
    parts: ClientParts,
    protocol: &'static dyn Protocol,
    hooks: Hooks,
    retry: Standard,
    region: Option<String>,
    timeouts: Timeouts,
}

/// A call ready to send: [`Client::prepare`]'s result, which owns all it
/// holds, so a caller can keep it beside its client and send it later
/// without preparing again.
#[derive(Debug)]
pub struct Prepared {
    operation: String,
    slots: Slots,
    request: HttpRequest,
    scheme: AuthScheme,
    /// The serialized request and the call's feature ids, to join again
    /// when the deferred account id changes the endpoint.
    serialized: Serialized,
    features: cli_core::client_config::user_agent::Features,
    input: Value,
    /// The endpoint the deferred account id left out.
    endpoint_url: String,
}

impl Client {
    /// A client speaking the protocol botocore resolves for the model.
    ///
    /// # Errors
    /// [`Unsupported::Protocol`] for one not implemented; see
    /// [`Self::with_protocol`].
    pub fn new(parts: ClientParts) -> Result<Self, CallError> {
        let protocol = protocol::for_model(&parts.model).ok_or_else(|| {
            let name = protocol::resolved_protocol(parts.model.metadata()).unwrap_or_default();
            CallError::Unsupported(Unsupported::Protocol(name.to_owned()))
        })?;
        Self::with_protocol(parts, protocol)
    }
    /// A client speaking `protocol`, its customizations chosen by scope
    /// and its retry settings resolved (`ClientArgsCreator`): the user's
    /// `max_attempts`, else a customization's default, else 3.
    ///
    /// # Errors
    /// `NoRegionError`, which botocore raises as it creates the client.
    pub fn with_protocol(
        parts: ClientParts,
        protocol: &'static dyn Protocol,
    ) -> Result<Self, CallError> {
        let hooks = Hooks::for_service(parts.customizations, &parts.service);
        let configured = parts
            .settings
            .max_attempts
            .as_ref()
            .map(|attempts| u32::try_from(attempts.value).unwrap_or(u32::MAX));
        let max_attempts = hooks
            .defaults()
            .max_attempts(configured)
            .unwrap_or(DEFAULT_MAX_ATTEMPTS);
        let base = hooks.defaults().retry_base_delay();
        let retry = Standard::new(max_attempts, base.unwrap_or(ErrorKind::Transient.base()));
        let region = parts.endpoint.client_region(&parts.endpoints)?;
        let timeouts = Timeouts {
            connect: timeout(parts.settings.connect_timeout_ms.value),
            read: timeout(parts.settings.read_timeout_ms.value),
        };
        Ok(Self {
            parts,
            protocol,
            hooks,
            retry,
            region,
            timeouts,
        })
    }
    /// Runs `operation` with `input`: [`Self::prepare`], then [`Self::send`].
    ///
    /// # Errors
    /// See [`CallError`].
    pub fn call(&self, operation: &str, input: Value) -> Result<Output, CallError> {
        let prepared = self.prepare(operation, input)?;
        self.send(prepared)
    }
    /// [`Self::call`], its `User-Agent` also naming `features`: the feature
    /// ids a caller registers in the call's context, as
    /// `PageIterator._make_request` registers `PAGINATOR`
    /// (`with_current_context(partial(register_feature_id, ...))`).
    ///
    /// # Errors
    /// See [`CallError`].
    pub fn call_with_features(
        &self,
        operation: &str,
        input: Value,
        features: Features,
    ) -> Result<Output, CallError> {
        let prepared = self.prepare_with_features(operation, input, features)?;
        self.send(prepared)
    }
    /// The service's model.
    pub(crate) fn model(&self) -> &Model {
        &self.parts.model
    }
}

/// A timeout in milliseconds as the transport takes it: 0, botocore's
/// "none", is the longest the transport can wait.
fn timeout(milliseconds: u32) -> Duration {
    match milliseconds {
        0 => Duration::from_millis(u64::from(u32::MAX)),
        ms => Duration::from_millis(u64::from(ms)),
    }
}

impl Client {
    /// Everything before the first attempt, in botocore's order: the
    /// static refusals ([`crate::admission::static_refusal`]), the
    /// `modify_input` stage (`provide-client-params`,
    /// `before-parameter-build`), idempotency tokens
    /// (`generate_idempotent_uuid`), the endpoint (`_resolve_endpoint_ruleset`),
    /// validation and serialization (`_convert_to_request_dict`), the join
    /// with the endpoint and the `User-Agent` (`prepare_request_dict`), the
    /// `after_serialize` stage, and auth selection.
    ///
    /// It needs no credentials, network or runtime, so every refusal comes
    /// before any credential provider runs: the endpoint resolves with
    /// `AWS::Auth::AccountId` deferred, as botocore defers it
    /// (`credentials.get_deferred_property`), and [`Self::send`] resolves
    /// again only when the credentials carry an account id the rule set
    /// reads.
    ///
    /// # Errors
    /// Anything botocore raises before it sends, and
    /// [`CallError::Unsupported`] for a call to leave to the Python route.
    pub fn prepare(&self, operation: &str, input: Value) -> Result<Prepared, CallError> {
        self.prepare_with_features(operation, input, Features::default())
    }
    /// [`Self::prepare`], with `extra` among the call's feature ids.
    fn prepare_with_features(
        &self,
        operation: &str,
        input: Value,
        extra: Features,
    ) -> Result<Prepared, CallError> {
        let parts = &self.parts;
        let operation = parts
            .model
            .operation(operation)
            .ok_or_else(|| CallError::UnknownOperation(operation.to_owned()))?;
        if let Some(unsupported) = crate::admission::static_refusal(
            &parts.model,
            operation,
            &parts.endpoints,
            &parts.service,
            parts.customizations,
            // The paginator's entry was judged where the call was admitted.
            None,
        ) {
            return Err(CallError::Unsupported(unsupported));
        }
        // What botocore's client creation raises once a region resolves.
        parts.endpoint.check(&parts.endpoints)?;
        let mut cx = CallContext::new(&parts.service, &parts.model, operation);
        let mut input = input;
        for hook in self.hooks.interceptors(&operation.name) {
            hook.modify_input(&mut cx, &mut input)
                .map_err(prepare_hook_error)?;
        }
        self.fill_idempotency_tokens(operation, &mut input);
        let (endpoint, mut features) = self.resolve_endpoint(&mut cx, operation, &input, None)?;
        features.extend(extra.iter());
        match validate_input(&parts.model, operation, &input) {
            Validation::Valid => {}
            Validation::Invalid(report) => return Err(CallError::Validation(report)),
            Validation::Undecided(undecided) => {
                return Err(CallError::Unsupported(Unsupported::Validation(undecided)));
            }
        }
        let serialized = self
            .protocol
            .serialize(&parts.model, operation, &input)
            .map_err(CallError::Serialize)?;
        let request = self.join_request(&mut cx, &serialized, &endpoint.url, features)?;
        let scheme = self.select_auth(&mut cx, operation, &endpoint)?;
        Ok(Prepared {
            operation: operation.name.to_string(),
            slots: cx.into_slots(),
            request,
            scheme,
            serialized,
            features,
            input,
            endpoint_url: endpoint.url,
        })
    }
    /// The serialized request joined to `url` with the call's
    /// `User-Agent`, after the `after_serialize` stage.
    fn join_request(
        &self,
        cx: &mut CallContext<'_>,
        serialized: &Serialized,
        url: &str,
        features: cli_core::client_config::user_agent::Features,
    ) -> Result<HttpRequest, CallError> {
        let parts = &self.parts;
        let user_agent = crate::useragent::header(&parts.settings, &parts.user_agent, features);
        let mut request = join(serialized.clone(), url, &user_agent)?;
        for hook in self.hooks.interceptors(&cx.operation.name) {
            hook.after_serialize(cx, &mut request)
                .map_err(prepare_hook_error)?;
        }
        Ok(request)
    }
    /// `generate_idempotent_uuid`: each input member marked
    /// `idempotencyToken` and left out gets `str(uuid.uuid4())`.
    fn fill_idempotency_tokens(&self, operation: &Operation, input: &mut Value) {
        let model = &self.parts.model;
        let Some(shape) = operation.input else {
            return;
        };
        let tokens: Vec<&str> = model
            .members(shape)
            .iter()
            .filter(|member| model.shape(member.shape).traits.is(Flag::IdempotencyToken))
            .map(|member| &*member.name)
            .collect();
        if tokens.is_empty() {
            return;
        }
        if *input == Value::Null {
            *input = Value::Map(Map::new());
        }
        if let Value::Map(map) = input {
            for name in tokens {
                if !map.contains_key(name) {
                    map.insert(name.to_owned(), Value::String(self.parts.runtime.uuid4()));
                }
            }
        }
    }
    /// The endpoint of a call of `operation` with `input`, the built-ins
    /// after the `endpoint_builtins` stage and the deferred account id
    /// `account_id`, and the feature ids resolving it registered.
    fn resolve_endpoint(
        &self,
        cx: &mut CallContext<'_>,
        operation: &Operation,
        input: &Value,
        account_id: Option<&str>,
    ) -> Result<(ResolvedEndpoint, Features), CallError> {
        let parts = &self.parts;
        let mut builtins = parts.endpoint.builtins(&parts.endpoints)?;
        for hook in self.hooks.interceptors(&operation.name) {
            hook.endpoint_builtins(cx, &mut builtins)
                .map_err(prepare_hook_error)?;
        }
        let params = endpoint::provider_params(
            &parts.endpoints,
            &parts.model,
            operation,
            input,
            &builtins,
            account_id,
        );
        let resolved = endpoint::resolve(&parts.endpoints, &params)?;
        Ok((resolved, crate::useragent::endpoint_features(&params)))
    }
    /// The account id the deferred `AWS::Auth::AccountId` reads, when the
    /// rule set reads it and the client signs: the credentials' own, which
    /// loads them as botocore's deferred property does.
    fn deferred_account_id(&self) -> Result<Option<String>, CallError> {
        let parts = &self.parts;
        if !parts.settings.signing.value || !endpoint::reads_account_id(&parts.endpoints) {
            return Ok(None);
        }
        let credentials = parts
            .credentials
            .credentials()
            .map_err(|error| CallError::Credentials(error.0))?;
        Ok(credentials.and_then(|credentials| credentials.account_id))
    }
    /// The scheme botocore's selection gives, unless a `select_auth`
    /// interceptor gives another.
    fn select_auth(
        &self,
        cx: &mut CallContext<'_>,
        operation: &Operation,
        endpoint: &ResolvedEndpoint,
    ) -> Result<AuthScheme, CallError> {
        let parts = &self.parts;
        let resolved = auth::select(
            &parts.model,
            &parts.service,
            operation,
            endpoint,
            parts.settings.signing.value,
            self.region.as_deref(),
        )?;
        let chosen = self
            .hooks
            .interceptors(&operation.name)
            .find_map(|hook| hook.select_auth(cx, &resolved));
        Ok(chosen.unwrap_or(resolved))
    }
}

/// A `prepare` stage's hook error: a refusal leaves the call to Python,
/// which is legal there because nothing has been sent and no credential
/// provider has run.
fn prepare_hook_error(error: HookError) -> CallError {
    match error {
        HookError::Refuse(reason) => CallError::Unsupported(Unsupported::Hook(reason)),
        other => CallError::Hook(other),
    }
}

/// A `prepare` stage's error when [`Client::send`] runs it again, after a
/// credential provider has run, too late to leave the call to Python: a
/// customization's refusal fails the call ([`HookError::after_prepare`]),
/// and any other refusal is a defect of the client, named as one.
fn after_prepare(error: CallError) -> CallError {
    match error {
        CallError::Unsupported(Unsupported::Hook(reason)) => {
            CallError::Hook(HookError::Refuse(reason).after_prepare())
        }
        CallError::Unsupported(unsupported) => CallError::Internal(format!(
            "the call was refused after credentials had loaded, which prepare should have \
             done: {unsupported}"
        )),
        other => other,
    }
}

/// `prepare_request_dict`: the serialized request joined to the endpoint
/// (`_urljoin`), its host prefix before the endpoint's host, its query
/// after the endpoint's, and `User-Agent` set after the serializer's
/// headers. Host prefix injection is always on: the router sends
/// `disable_host_prefix_injection` to Python.
fn join(
    serialized: Serialized,
    endpoint: &str,
    user_agent: &str,
) -> Result<HttpRequest, CallError> {
    let mut url = Url::parse(endpoint)
        .ok_or_else(|| CallError::Endpoint(format!("Invalid endpoint: {endpoint}")))?;
    if let Some(prefix) = &serialized.host_prefix {
        url.authority.insert_str(0, prefix);
    }
    let path = &serialized.path;
    if !(path.is_empty() || path == "/") {
        match (url.path.strip_suffix('/'), path.starts_with('/')) {
            (Some(base), true) => url.path = format!("{base}{path}"),
            _ => url.path.push_str(path),
        }
    }
    if !serialized.query.is_empty() {
        if !url.query.is_empty() {
            url.query.push('&');
        }
        url.query.push_str(&serialized.query);
    }
    let mut headers = serialized.headers;
    headers.set("User-Agent", user_agent);
    Ok(HttpRequest {
        method: serialized.method,
        url,
        headers,
        body: serialized.body,
    })
}

/// What one attempt decided.
enum Step {
    Done(Result<Output, CallError>),
    Retry {
        delay: Duration,
        cost: Option<u32>,
        /// The response's `Date`, which sets the next attempt's `ttl`.
        date: Option<timestamp::Timestamp>,
    },
}

impl Client {
    /// Sends `prepared`, retrying as standard mode does
    /// (`Endpoint._send_request`).
    ///
    /// # Errors
    /// See [`CallError`].
    pub fn send(&self, prepared: Prepared) -> Result<Output, CallError> {
        let Prepared {
            operation,
            slots,
            mut request,
            mut scheme,
            serialized,
            features,
            input,
            endpoint_url,
        } = prepared;
        let parts = &self.parts;
        let operation = parts
            .model
            .operation(&operation)
            .ok_or_else(|| CallError::UnknownOperation(operation.clone()))?;
        let mut cx = CallContext::resume(&parts.service, &parts.model, operation, slots);
        if let Some(account_id) = self.deferred_account_id()? {
            // The endpoint as the account id gives it; the refusals it could
            // meet were made on the same rule set in prepare
            // (`account_id_endpoints_refuse_nothing_more` pins it), so one
            // here is a defect, not a reason to leave the call to Python.
            let (endpoint, resolved_features) = self
                .resolve_endpoint(&mut cx, operation, &input, Some(&account_id))
                .map_err(after_prepare)?;
            if endpoint.url != endpoint_url || resolved_features != features {
                request = self
                    .join_request(&mut cx, &serialized, &endpoint.url, resolved_features)
                    .map_err(after_prepare)?;
                scheme = self
                    .select_auth(&mut cx, operation, &endpoint)
                    .map_err(after_prepare)?;
            }
        }
        let invocation = self.parts.runtime.uuid4();
        let mut ttl: Option<String> = None;
        let mut last_cost = None;
        let mut attempt = 1;
        loop {
            cx.attempt = attempt;
            let sent = self.attempt_request(
                &mut cx,
                &request,
                &scheme,
                features,
                &invocation,
                ttl.as_deref(),
            )?;
            let result = self.transmit(&mut cx, &sent)?;
            match self.evaluate(&mut cx, attempt, &sent, result, last_cost) {
                Step::Done(outcome) => return outcome,
                Step::Retry { delay, cost, date } => {
                    if let Some(date) = date {
                        ttl = Some(self.ttl(date));
                    }
                    self.parts
                        .runtime
                        .sleep(delay)
                        .map_err(|_| CallError::Interrupted)?;
                    last_cost = cost;
                    attempt += 1;
                }
            }
        }
    }
    /// One attempt's request: the `before_sign` stage, the signature, then
    /// the unsigned headers (`add_retry_headers`, `_prepare_headers`).
    fn attempt_request(
        &self,
        cx: &mut CallContext<'_>,
        request: &HttpRequest,
        scheme: &AuthScheme,
        features: cli_core::client_config::user_agent::Features,
        invocation: &str,
        ttl: Option<&str>,
    ) -> Result<HttpRequest, CallError> {
        let mut request = request.clone();
        for hook in self.hooks.interceptors(&cx.operation.name) {
            hook.before_sign(cx, &mut request)
                .map_err(|error| CallError::Hook(error.after_prepare()))?;
        }
        if let AuthScheme::SigV4 {
            signing_region,
            signing_name,
        } = scheme
        {
            let credentials = self
                .parts
                .credentials
                .credentials()
                .map_err(|error| CallError::Credentials(error.0))?
                .ok_or(CallError::NoCredentials)?;
            // The provider registers its feature ids as it answers, and
            // `rebuild_and_replace_user_agent_handler` rebuilds the header
            // after the signer, which does not sign `User-Agent`.
            if let Some(method) = self.parts.credentials.method() {
                let mut features = features;
                features.extend(
                    cli_core::client_config::user_agent::credential_features(method)
                        .iter()
                        .copied(),
                );
                let parts = &self.parts;
                let agent = crate::useragent::header(&parts.settings, &parts.user_agent, features);
                request.headers.set("User-Agent", agent);
            }
            let now = self.parts.runtime.now();
            auth::sign(
                signing_region,
                signing_name,
                &credentials,
                now,
                &mut request,
            );
        }
        request.headers.set("amz-sdk-invocation-id", invocation);
        let ttl = ttl.map(|ttl| format!("ttl={ttl}; ")).unwrap_or_default();
        let max = self.retry.max_attempts();
        request.headers.set(
            "amz-sdk-request",
            format!("{ttl}attempt={}; max={max}", cx.attempt),
        );
        let framed = !matches!(request.method, Method::Get | Method::Head | Method::Options);
        if framed && !request.headers.contains("Content-Length") {
            request
                .headers
                .set("Content-Length", request.body.len().to_string());
        }
        Ok(request)
    }
    /// The `before_transmit` stage's response, else the transport's.
    fn transmit(
        &self,
        cx: &mut CallContext<'_>,
        request: &HttpRequest,
    ) -> Result<Result<HttpResponse, TransportError>, CallError> {
        for hook in self.hooks.interceptors(&cx.operation.name) {
            if let Some(response) = hook
                .before_transmit(cx, request)
                .map_err(|error| CallError::Hook(error.after_prepare()))?
            {
                return Ok(Ok(response));
            }
        }
        Ok(self.parts.transport.send(request, self.timeouts))
    }
    /// `_update_retries_context`'s `ttl` (`Endpoint._calculate_ttl`): now,
    /// plus the read timeout, plus the server's clock skew.
    fn ttl(&self, date: timestamp::Timestamp) -> String {
        let now = self.parts.runtime.now();
        let read = i64::try_from(self.timeouts.read.as_micros()).unwrap_or(i64::MAX);
        let skew = date.unix_micros() - now.unix_micros();
        let micros = now.unix_micros().saturating_add(read).saturating_add(skew);
        timestamp::Timestamp::from_unix_micros(micros)
            .map_or_else(String::new, |ttl| ttl.iso8601_basic())
    }
}

impl Client {
    /// What one attempt's result decides: botocore's `needs-retry`, after
    /// the response is parsed, or the call's result.
    fn evaluate(
        &self,
        cx: &mut CallContext<'_>,
        attempt: u32,
        request: &HttpRequest,
        result: Result<HttpResponse, TransportError>,
        last_cost: Option<u32>,
    ) -> Step {
        let mut response = match result {
            Ok(response) => response,
            Err(TransportError::Interrupted) => return Step::Done(Err(CallError::Interrupted)),
            Err(error) => return self.failed(cx, attempt, request, error),
        };
        for hook in self.hooks.interceptors(&cx.operation.name) {
            if let Err(error) = hook.after_receive(cx, &mut response) {
                return Step::Done(Err(CallError::Hook(error.after_prepare())));
            }
        }
        // A body neither parse can read is `ResponseParserError`, which
        // botocore raises before `needs-retry`.
        let (model, options) = (&self.parts.model, &self.parts.parse.options());
        if response.status >= 300 {
            return match self
                .protocol
                .parse_error(model, cx.operation, &response, options)
            {
                Err(error) => Step::Done(Err(CallError::Parse(error))),
                Ok(error) => self.service_error(cx, attempt, &response, error),
            };
        }
        match self.protocol.parse(model, cx.operation, &response, options) {
            Err(error) => Step::Done(Err(CallError::Parse(error))),
            // `parsed_response.update(customized_response_dict)` in
            // `Endpoint._do_get_response`: a parse to `None` raises there.
            Ok(Value::Null) => Step::Done(Err(CallError::Parse(ParseError::new(
                "'NoneType' object has no attribute 'update'",
            )))),
            Ok(value) => self.succeeded(cx, attempt, response, value, last_cost),
        }
    }
    /// The retry kind of an attempt: a `classify_retry` interceptor's
    /// decision, else standard mode's.
    ///
    /// # Errors
    /// An interceptor's check that raised ([`RetryDecision::Fail`]).
    fn classify(
        &self,
        cx: &mut CallContext<'_>,
        attempt: &Attempt<'_>,
    ) -> Result<Option<ErrorKind>, CallError> {
        let decided = self
            .hooks
            .interceptors(&cx.operation.name)
            .find_map(|hook| hook.classify_retry(cx, attempt));
        Ok(match decided {
            Some(RetryDecision::Stop) => None,
            Some(RetryDecision::Retry) => Some(
                retry::classify(&self.parts.model, cx.operation, attempt)
                    .unwrap_or(ErrorKind::Transient),
            ),
            Some(RetryDecision::Fail(error)) => {
                return Err(CallError::Hook(error.after_prepare()));
            }
            None => retry::classify(&self.parts.model, cx.operation, attempt),
        })
    }
    /// `RetryHandler.needs_retry` for a long-polling operation whose retry
    /// only the quota refused: it sleeps the delay it would have waited.
    fn long_poll_wait(
        &self,
        cx: &CallContext<'_>,
        verdict: &retry::Verdict,
    ) -> Result<(), CallError> {
        if let (None, Some(delay)) = (verdict.delay, verdict.exhausted_delay)
            && self.hooks.long_polls(&cx.operation.name)
        {
            self.parts
                .runtime
                .sleep(delay)
                .map_err(|_| CallError::Interrupted)?;
        }
        Ok(())
    }
    fn failed(
        &self,
        cx: &mut CallContext<'_>,
        attempt: u32,
        request: &HttpRequest,
        error: TransportError,
    ) -> Step {
        let kind = match self.classify(cx, &Attempt::Failed(&error)) {
            Ok(kind) => kind,
            Err(failure) => return Step::Done(Err(failure)),
        };
        let verdict = self
            .retry
            .needs_retry(attempt, kind, None, self.parts.runtime.random());
        if let Err(error) = self.long_poll_wait(cx, &verdict) {
            return Step::Done(Err(error));
        }
        match verdict.delay {
            Some(delay) => Step::Retry {
                delay,
                cost: verdict.retry_cost,
                date: None,
            },
            None => Step::Done(Err(CallError::Transport {
                error,
                endpoint_url: request.url.to_string(),
            })),
        }
    }
    fn service_error(
        &self,
        cx: &mut CallContext<'_>,
        attempt: u32,
        response: &HttpResponse,
        mut error: ServiceError,
    ) -> Step {
        let attempted = Attempt::Response {
            response,
            error: Some(&error),
        };
        let kind = match self.classify(cx, &attempted) {
            Ok(kind) => kind,
            Err(failure) => return Step::Done(Err(failure)),
        };
        let retry_after = retry::retry_after(&response.headers);
        let verdict =
            self.retry
                .needs_retry(attempt, kind, retry_after, self.parts.runtime.random());
        if let Err(error) = self.long_poll_wait(cx, &verdict) {
            return Step::Done(Err(error));
        }
        if let Some(delay) = verdict.delay {
            return Step::Retry {
                delay,
                cost: verdict.retry_cost,
                date: date_of(response),
            };
        }
        error.retry_attempts = attempt - 1;
        error.max_attempts_reached = verdict.max_attempts_reached;
        error.headers = response.headers.clone();
        Step::Done(Err(CallError::Service(Box::new(error))))
    }
    fn succeeded(
        &self,
        cx: &mut CallContext<'_>,
        attempt: u32,
        response: HttpResponse,
        mut value: Value,
        last_cost: Option<u32>,
    ) -> Step {
        let attempted = Attempt::Response {
            response: &response,
            error: None,
        };
        let kind = match self.classify(cx, &attempted) {
            Ok(kind) => kind,
            Err(failure) => return Step::Done(Err(failure)),
        };
        let verdict = self
            .retry
            .needs_retry(attempt, kind, None, self.parts.runtime.random());
        if let Err(error) = self.long_poll_wait(cx, &verdict) {
            return Step::Done(Err(error));
        }
        if let Some(delay) = verdict.delay {
            return Step::Retry {
                delay,
                cost: verdict.retry_cost,
                date: date_of(&response),
            };
        }
        self.retry.succeeded(last_cost);
        for hook in self.hooks.interceptors(&cx.operation.name) {
            if let Err(error) = hook.modify_output(cx, &mut value) {
                return Step::Done(Err(CallError::Hook(error.after_prepare())));
            }
        }
        Step::Done(Ok(Output {
            value,
            status: response.status,
            headers: response.headers,
            retry_attempts: attempt - 1,
        }))
    }
}

/// A response's `Date`, as `_calculate_ttl` reads it.
fn date_of(response: &HttpResponse) -> Option<timestamp::Timestamp> {
    timestamp::Timestamp::parse_rfc822(response.headers.get("Date")?)
}

#[cfg(test)]
mod tests;
