//! The send phases: the deferred account id, then the attempts, retried
//! as standard mode does (`Endpoint._send_request`).
use super::prepare::{
    apply_endpoint, modify_before_retry_loop, resolve_auth_scheme, resolve_endpoint,
};
use super::{after_prepare, find};
use crate::auth::{self, AuthScheme};
use crate::call::{Client, Output, Prepared};
use crate::credentials::SigningCredentials;
use crate::endpoint;
use crate::error::{CallError, ParseError, ServiceError};
use crate::http::{HttpRequest, HttpResponse, Method};
use crate::interceptors::InterceptorContext;
use crate::retry::{self, Attempt, ErrorKind, RetryDecision};
use crate::transport::TransportError;
use cli_core::client_config::user_agent::Features;
use model::value::Value;
use std::time::Duration;

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

/// What an attempt received, once deserialized.
enum Received {
    /// A transport error other than an interruption.
    Failed(TransportError),
    /// A response of status 300 or more, parsed as a service error.
    Error {
        response: HttpResponse,
        error: ServiceError,
    },
    Parsed {
        response: HttpResponse,
        value: Value,
    },
}

impl Received {
    /// The attempt as retry classification sees it.
    fn attempt(&self) -> Attempt<'_> {
        match self {
            Self::Failed(error) => Attempt::Failed(error),
            Self::Error { response, error } => Attempt::Response {
                response,
                error: Some(error),
            },
            Self::Parsed { response, .. } => Attempt::Response {
                response,
                error: None,
            },
        }
    }
    /// The service error's `Retry-After`; standard mode reads no other.
    fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Error { response, .. } => retry::retry_after(&response.headers),
            Self::Failed(_) | Self::Parsed { .. } => None,
        }
    }
    /// A response's `Date`, for the next attempt's `ttl`.
    fn date(&self) -> Option<timestamp::Timestamp> {
        match self {
            Self::Failed(_) => None,
            Self::Error { response, .. } | Self::Parsed { response, .. } => date_of(response),
        }
    }
}

/// [`Client::send`]: the send phases in order.
pub(crate) fn send(client: &Client, mut prepared: Prepared) -> Result<Output, CallError> {
    let operation = find(&client.model, &prepared.operation)?;
    let slots = std::mem::take(&mut prepared.slots);
    let service_id = client.service_id.as_deref();
    let mut ctx = InterceptorContext::resume(&client.service, service_id, operation, slots);
    refresh_for_identity(client, &mut ctx, &mut prepared)?;
    let invocation = client.components.runtime.uuid4();
    let mut ttl: Option<String> = None;
    let mut last_cost = None;
    let mut attempt = 1;
    loop {
        ctx.attempt = attempt;
        let sent = attempt_request(client, &mut ctx, &prepared, &invocation, ttl.as_deref())?;
        let result = transmit(client, &mut ctx, &sent)?;
        match evaluate(client, &mut ctx, attempt, &sent, result, last_cost) {
            Step::Done(outcome) => return outcome,
            Step::Retry { delay, cost, date } => {
                if let Some(date) = date {
                    ttl = Some(self::ttl(client, date));
                }
                (client.components.runtime)
                    .sleep(delay)
                    .map_err(|_| CallError::Interrupted)?;
                last_cost = cost;
                attempt += 1;
            }
        }
    }
}

/// The endpoint as the deferred account id gives it, when the rule set
/// reads `AWS::Auth::AccountId` and the client signs; when it changes the
/// url or the feature ids, the request is joined, its
/// `modify_before_retry_loop` hooks run and its scheme selected again.
fn refresh_for_identity(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    prepared: &mut Prepared,
) -> Result<(), CallError> {
    let Some(account_id) = deferred_account_id(client)? else {
        return Ok(());
    };
    // The refusals it could meet were made on the same rule set in prepare
    // (`account_id_endpoints_refuse_nothing_more` pins it), so one here is
    // a defect, not a reason to leave the call to Python.
    let (endpoint, features) =
        resolve_endpoint(client, ctx, &prepared.input, Some(&account_id)).map_err(after_prepare)?;
    if endpoint.url != prepared.endpoint_url || features != prepared.features {
        let mut request = apply_endpoint(client, &prepared.serialized, &endpoint.url, features)
            .map_err(after_prepare)?;
        modify_before_retry_loop(client, ctx, &mut request).map_err(after_prepare)?;
        prepared.request = request;
        prepared.scheme =
            resolve_auth_scheme(client, ctx.operation, &endpoint).map_err(after_prepare)?;
    }
    Ok(())
}

/// The account id the deferred `AWS::Auth::AccountId` reads, when the
/// rule set reads it and the client signs: the credentials' own, which
/// loads them as botocore's deferred property does.
fn deferred_account_id(client: &Client) -> Result<Option<String>, CallError> {
    if !client.settings.signing.value
        || !endpoint::reads_account_id(&client.components.endpoint_resolver.data)
    {
        return Ok(None);
    }
    let credentials = (client.components.identity_resolver)
        .credentials()
        .map_err(|error| CallError::Credentials(error.0))?;
    Ok(credentials.and_then(|credentials| credentials.account_id))
}

/// One attempt's request: `modify_before_signing`, then for a SigV4
/// scheme the identity and the signature, then `modify_before_transmit`.
fn attempt_request(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    prepared: &Prepared,
    invocation: &str,
    ttl: Option<&str>,
) -> Result<HttpRequest, CallError> {
    let mut request = prepared.request.clone();
    modify_before_signing(client, ctx, &mut request)?;
    if let AuthScheme::SigV4 {
        signing_region,
        signing_name,
    } = &prepared.scheme
    {
        let credentials = resolve_identity(client)?;
        let signer = Signer {
            region: signing_region,
            name: signing_name,
            credentials: &credentials,
        };
        sign(client, &signer, prepared.features, &mut request);
    }
    modify_before_transmit(client, ctx, &mut request, invocation, ttl)?;
    Ok(request)
}

/// The `modify_before_signing` hooks (`before-sign`). An error ends the
/// call without a retry, and wins over a credentials error: the identity
/// comes after it.
fn modify_before_signing(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    request: &mut HttpRequest,
) -> Result<(), CallError> {
    for hook in client.components.plugins.interceptors(ctx.operation.name()) {
        hook.modify_before_signing(ctx, request)
            .map_err(|error| CallError::Hook(error.after_prepare()))?;
    }
    Ok(())
}

/// The credentials, asked of the source on every attempt: the source
/// refreshes what is due (during a backoff too), and nothing here caches.
/// An error, or none, ends the call without a retry.
fn resolve_identity(client: &Client) -> Result<SigningCredentials, CallError> {
    (client.components.identity_resolver)
        .credentials()
        .map_err(|error| CallError::Credentials(error.0))?
        .ok_or(CallError::NoCredentials)
}

/// What one SigV4 signature is made with.
struct Signer<'a> {
    region: &'a str,
    name: &'a str,
    credentials: &'a SigningCredentials,
}

/// The `User-Agent` with the credential source's feature ids, then the
/// signature. The provider registers its feature ids as it answers, and
/// `rebuild_and_replace_user_agent_handler` rebuilds the header after the
/// signer, which does not sign `User-Agent`.
fn sign(client: &Client, signer: &Signer<'_>, features: Features, request: &mut HttpRequest) {
    if let Some(method) = client.components.identity_resolver.method() {
        let mut features = features;
        features.extend(
            cli_core::client_config::user_agent::credential_features(method)
                .iter()
                .copied(),
        );
        let agent = crate::useragent::header(&client.settings, &client.user_agent, features);
        request.headers.set("User-Agent", agent);
    }
    let now = client.components.runtime.now();
    auth::sign(signer.region, signer.name, signer.credentials, now, request);
}

/// The core's `modify_before_transmit`: the unsigned headers, after the
/// signature (`add_retry_headers`, with `max` as `seed_max_attempts`
/// seeds it, then `_prepare_headers`'s `Content-Length`); then any
/// customization's, whose error ends the call without a retry.
fn modify_before_transmit(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    request: &mut HttpRequest,
    invocation: &str,
    ttl: Option<&str>,
) -> Result<(), CallError> {
    let attempt = ctx.attempt;
    request.headers.set("amz-sdk-invocation-id", invocation);
    let ttl = ttl.map(|ttl| format!("ttl={ttl}; ")).unwrap_or_default();
    let max = client.components.retry_strategy.max_attempts();
    request.headers.set(
        "amz-sdk-request",
        format!("{ttl}attempt={attempt}; max={max}"),
    );
    let framed = !matches!(request.method, Method::Get | Method::Head | Method::Options);
    if framed && !request.headers.contains("Content-Length") {
        request
            .headers
            .set("Content-Length", request.body.len().to_string());
    }
    for hook in client.components.plugins.interceptors(ctx.operation.name()) {
        hook.modify_before_transmit(ctx, request)
            .map_err(|error| CallError::Hook(error.after_prepare()))?;
    }
    Ok(())
}

/// The `short_circuit_transmit` hooks (botocore's `before-send`): the
/// first hook's response is the attempt's, and nothing is sent. A hook
/// error ends the call without a retry.
fn short_circuit_transmit(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    request: &HttpRequest,
) -> Result<Option<HttpResponse>, CallError> {
    for hook in client.components.plugins.interceptors(ctx.operation.name()) {
        let answered = hook
            .short_circuit_transmit(ctx, request)
            .map_err(|error| CallError::Hook(error.after_prepare()))?;
        if answered.is_some() {
            return Ok(answered);
        }
    }
    Ok(None)
}

/// [`short_circuit_transmit`]'s response, else the transport's.
fn transmit(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    request: &HttpRequest,
) -> Result<Result<HttpResponse, TransportError>, CallError> {
    if let Some(response) = short_circuit_transmit(client, ctx, request)? {
        return Ok(Ok(response));
    }
    Ok(client.components.http_client.send(request, client.timeouts))
}

/// `_update_retries_context`'s `ttl` (`Endpoint._calculate_ttl`): now,
/// plus the read timeout, plus the server's clock skew.
fn ttl(client: &Client, date: timestamp::Timestamp) -> String {
    let now = client.components.runtime.now();
    let read = i64::try_from(client.timeouts.read.as_micros()).unwrap_or(i64::MAX);
    let skew = date.unix_micros() - now.unix_micros();
    let micros = now.unix_micros().saturating_add(read).saturating_add(skew);
    timestamp::Timestamp::from_unix_micros(micros)
        .map_or_else(String::new, |ttl| ttl.iso8601_basic())
}

/// What one attempt's result decides: `deserialize`, `classify`, then
/// botocore's `needs-retry`, or the call's result.
///
/// The classify rule: only a transport error other than an interruption,
/// a parsed service error or a parsed response is classified, and only a
/// classified attempt can be retried or wait. An interruption, a
/// `modify_before_deserialization` hook error and a parse error end the
/// call here, as a `modify_before_signing`, `modify_before_transmit` or
/// `short_circuit_transmit` hook error, a credentials error and
/// `NoCredentials` ended it before the attempt was sent.
fn evaluate(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    attempt: u32,
    request: &HttpRequest,
    result: Result<HttpResponse, TransportError>,
    last_cost: Option<u32>,
) -> Step {
    let received = match deserialize(client, ctx, result) {
        Ok(received) => received,
        Err(error) => return Step::Done(Err(error)),
    };
    let kind = match classify(client, ctx, &received.attempt()) {
        Ok(kind) => kind,
        Err(failure) => return Step::Done(Err(failure)),
    };
    let random = client.components.runtime.random();
    let retry = &client.components.retry_strategy;
    let verdict = retry.needs_retry(attempt, kind, received.retry_after(), random);
    if let Err(error) = long_poll_wait(client, ctx, &verdict) {
        return Step::Done(Err(error));
    }
    if let Some(delay) = verdict.delay {
        return Step::Retry {
            delay,
            cost: verdict.retry_cost,
            date: received.date(),
        };
    }
    Step::Done(match received {
        Received::Failed(error) => Err(CallError::Transport {
            error,
            endpoint_url: request.url.to_string(),
        }),
        Received::Error {
            response,
            mut error,
        } => {
            error.retry_attempts = attempt - 1;
            error.max_attempts_reached = verdict.max_attempts_reached;
            error.headers = response.headers;
            Err(CallError::Service(Box::new(error)))
        }
        Received::Parsed { response, value } => {
            client.succeeded(ctx, attempt, response, value, last_cost)
        }
    })
}

/// The `modify_before_deserialization` hooks (`before-parse`), then the
/// parsers. An interruption, a hook error and a body neither parse can
/// read (`ResponseParserError`, which botocore raises before
/// `needs-retry`) end the call without a retry.
fn deserialize(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    result: Result<HttpResponse, TransportError>,
) -> Result<Received, CallError> {
    let mut response = match result {
        Ok(response) => response,
        Err(TransportError::Interrupted) => return Err(CallError::Interrupted),
        Err(error) => return Ok(Received::Failed(error)),
    };
    for hook in client.components.plugins.interceptors(ctx.operation.name()) {
        hook.modify_before_deserialization(ctx, &mut response)
            .map_err(|error| CallError::Hook(error.after_prepare()))?;
    }
    let (model, options) = (&client.model, &client.parse.options());
    if response.status >= 300 {
        let error = (client.components.protocol)
            .parse_error(ctx.operation, &response, options, model.schema().registry())
            .map_err(CallError::Parse)?;
        return Ok(Received::Error { response, error });
    }
    match client
        .components
        .protocol
        .parse(ctx.operation, &response, options)
    {
        Err(error) => Err(CallError::Parse(error)),
        // `parsed_response.update(customized_response_dict)` in
        // `Endpoint._do_get_response`: a parse to `None` raises there.
        Ok(Value::Null) => Err(CallError::Parse(ParseError::new(
            "'NoneType' object has no attribute 'update'",
        ))),
        Ok(value) => Ok(Received::Parsed { response, value }),
    }
}

/// The retry kind of an attempt: the first decision of a customization's
/// retry classifier (a service's `needs-retry` checker), else standard
/// mode's.
///
/// # Errors
/// A classifier's check that raised ([`RetryDecision::Fail`]).
fn classify(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    attempt: &Attempt<'_>,
) -> Result<Option<ErrorKind>, CallError> {
    let decided = (client.components.plugins)
        .retry_classifiers(ctx.operation.name())
        .find_map(|classifier| classifier.classify_retry(ctx, attempt));
    let model = &client.model;
    Ok(match decided {
        Some(RetryDecision::Stop) => None,
        Some(RetryDecision::Retry) => {
            Some(retry::classify(model, ctx.operation, attempt).unwrap_or(ErrorKind::Transient))
        }
        Some(RetryDecision::Fail(error)) => {
            return Err(CallError::Hook(error.after_prepare()));
        }
        None => retry::classify(model, ctx.operation, attempt),
    })
}

/// `RetryHandler.needs_retry` for a long-polling operation (a
/// customization's `long_polling`) whose retry only the quota refused: it
/// sleeps the delay it would have waited.
fn long_poll_wait(
    client: &Client,
    ctx: &InterceptorContext<'_>,
    verdict: &retry::Verdict,
) -> Result<(), CallError> {
    if let (None, Some(delay)) = (verdict.delay, verdict.exhausted_delay)
        && client.components.plugins.long_polls(ctx.operation.name())
    {
        (client.components.runtime)
            .sleep(delay)
            .map_err(|_| CallError::Interrupted)?;
    }
    Ok(())
}

/// A response's `Date`, as `_calculate_ttl` reads it from the
/// `HeadersDict`: a repeated header's values joined, which no date reads.
fn date_of(response: &HttpResponse) -> Option<timestamp::Timestamp> {
    timestamp::Timestamp::parse_rfc822(&response.headers.joined("Date")?)
}
