//! The prepare phases: everything that needs no credentials, network or
//! runtime, so every refusal comes before any credential provider runs.
use super::{find, prepare_hook_error};
use crate::admission::Unsupported;
use crate::auth::{self, AuthSchemeOptionResolverParams, SelectedAuth};
use crate::call::{Client, Prepared};
use crate::checksum::{self, CallChecksums};
use crate::endpoint::{self, ResolvedEndpoint};
use crate::error::CallError;
use crate::http::HttpRequest;
use crate::interceptors::InterceptorContext;
use crate::protocol::Serialized;
use cli_core::client_config::user_agent::Features;
use model::schema::{Flag, OperationSchema};
use model::validate::{Validation, validate_input};
use model::value::{Map, Value};

/// [`Client::prepare`] with `extra` among the call's feature ids: the
/// prepare phases in order, checkpointed into a [`Prepared`].
pub(crate) fn prepare(
    client: &Client,
    operation: &str,
    input: Value,
    extra: Features,
) -> Result<Prepared, CallError> {
    let operation = find(&client.model, operation)?;
    admit(client, operation)?;
    let mut ctx = InterceptorContext::new(&client.service, client.service_id.as_deref(), operation);
    let mut input = input;
    modify_before_serialization(client, &mut ctx, &mut input)?;
    fill_idempotency_tokens(client, operation, &mut input);
    if client.http_checksum {
        checksum::default_validation_mode(operation, &client.settings, &mut input);
    }
    let (endpoint, mut features) = resolve_endpoint(client, &mut ctx, &input, None)?;
    features.extend(extra.iter());
    let serialized = serialize(client, operation, &input)?;
    features.extend(resolve_checksums(client, &mut ctx, &input, &serialized, &endpoint)?.iter());
    let mut request = apply_endpoint(client, &serialized, &endpoint.url, features)?;
    modify_before_retry_loop(client, &mut ctx, &mut request)?;
    apply_checksums(client, &mut ctx, &mut request);
    let auth = resolve_auth_scheme(client, operation, &endpoint)?;
    Ok(Prepared {
        operation: operation.name().to_owned(),
        slots: ctx.into_state(),
        request,
        auth,
        serialized,
        features,
        input,
        endpoint_url: endpoint.url,
    })
}

/// The static refusals ([`crate::admission::static_refusal`]), then what
/// botocore's client creation raises once a region resolves.
fn admit(client: &Client, operation: OperationSchema<'_>) -> Result<(), CallError> {
    if let Some(unsupported) = crate::admission::refusal(
        &client.model,
        operation,
        &client.components.endpoint_resolver.data,
        &client.service,
        client.customizations,
        // The paginator's entry was judged where the call was admitted.
        None,
        client.http_checksum,
    ) {
        return Err(CallError::Unsupported(unsupported));
    }
    let resolver = &client.components.endpoint_resolver;
    resolver.config.check(&resolver.data)?;
    Ok(())
}

/// The `modify_before_serialization` hooks (`provide-client-params`,
/// `before-parameter-build`).
fn modify_before_serialization(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    input: &mut Value,
) -> Result<(), CallError> {
    for hook in client.components.plugins.interceptors(ctx.operation.name()) {
        hook.modify_before_serialization(ctx, input)
            .map_err(prepare_hook_error)?;
    }
    Ok(())
}

/// `generate_idempotent_uuid`: each input member marked
/// `idempotencyToken` and left out gets `str(uuid.uuid4())`.
fn fill_idempotency_tokens(client: &Client, operation: OperationSchema<'_>, input: &mut Value) {
    let Some(shape) = operation.input() else {
        return;
    };
    let tokens: Vec<&str> = shape
        .members()
        .filter(|member| member.flag(Flag::IdempotencyToken))
        .filter_map(|member| member.member_name())
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
                map.insert(
                    name.to_owned(),
                    Value::String(client.components.runtime.uuid4()),
                );
            }
        }
    }
}

/// The endpoint of a call with `input` (`_resolve_endpoint_ruleset`), the
/// built-ins after the `modify_endpoint_builtins` hooks
/// (`before-endpoint-resolution`) and the deferred account id
/// `account_id`, and the feature ids resolving it registered.
pub(super) fn resolve_endpoint(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    input: &Value,
    account_id: Option<&str>,
) -> Result<(ResolvedEndpoint, Features), CallError> {
    let (resolver, operation) = (&client.components.endpoint_resolver, ctx.operation);
    let mut builtins = resolver.config.builtins(&resolver.data)?;
    for hook in client.components.plugins.interceptors(ctx.operation.name()) {
        hook.modify_endpoint_builtins(ctx, &mut builtins)
            .map_err(prepare_hook_error)?;
    }
    let params = endpoint::provider_params(&resolver.data, operation, input, &builtins, account_id);
    let resolved = resolver.resolve(&params)?;
    Ok((resolved, crate::useragent::endpoint_features(&params)))
}

/// Validation, then the protocol's serializer (`_convert_to_request_dict`):
/// an endpoint error wins over a validation error.
fn serialize(
    client: &Client,
    operation: OperationSchema<'_>,
    input: &Value,
) -> Result<Serialized, CallError> {
    match validate_input(operation, input) {
        Validation::Valid => {}
        Validation::Invalid(report) => return Err(CallError::Validation(report)),
        Validation::Undecided(undecided) => {
            return Err(CallError::Unsupported(Unsupported::Validation(undecided)));
        }
    }
    (client.components.protocol)
        .serialize_request(operation, input, &client.model)
        .map_err(CallError::Serialize)
}

/// The serialized request joined to `url` with the call's `User-Agent`
/// (`prepare_request_dict`).
pub(super) fn apply_endpoint(
    client: &Client,
    serialized: &Serialized,
    url: &str,
    features: Features,
) -> Result<HttpRequest, CallError> {
    let user_agent = crate::useragent::header(&client.settings, &client.user_agent, features);
    (client.components.protocol).update_endpoint(serialized, url, &user_agent)
}

/// The `modify_before_retry_loop` hooks (`before-call`), once per call
/// (and again when the account id changes the endpoint).
pub(super) fn modify_before_retry_loop(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    request: &mut HttpRequest,
) -> Result<(), CallError> {
    for hook in client.components.plugins.interceptors(ctx.operation.name()) {
        hook.modify_before_retry_loop(ctx, request)
            .map_err(prepare_hook_error)?;
    }
    Ok(())
}

/// `resolve_checksum_context` for a client with checksums switched on:
/// the call's checksums, kept for [`apply_checksums`] and the responses,
/// and the request feature ids they register.
fn resolve_checksums(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    input: &Value,
    serialized: &Serialized,
    endpoint: &ResolvedEndpoint,
) -> Result<Features, CallError> {
    if !client.http_checksum {
        return Ok(Features::default());
    }
    let (checksums, features) = checksum::resolve(
        ctx.operation,
        &client.settings,
        input,
        &serialized.headers,
        &endpoint.url,
    )?;
    *ctx.state::<CallChecksums>() = checksums;
    *ctx.state::<ChecksumFeatures>() = ChecksumFeatures(features);
    Ok(features)
}

/// The request feature ids [`resolve_checksums`] registered, which an
/// endpoint the deferred account id changes keeps.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ChecksumFeatures(pub(super) Features);

/// `apply_request_checksum`, after the `modify_before_retry_loop` hooks
/// (botocore's `before-call`), for a client with checksums switched on.
pub(super) fn apply_checksums(
    client: &Client,
    ctx: &mut InterceptorContext<'_>,
    request: &mut HttpRequest,
) {
    if client.http_checksum {
        ctx.state::<CallChecksums>().apply(request);
    }
}

/// The auth option botocore's selection gives (`choose-signer`), from the
/// client's option resolver, and the client's scheme that serves it: the
/// components, not a hook. Either may refuse, before any credential.
pub(super) fn resolve_auth_scheme(
    client: &Client,
    operation: OperationSchema<'_>,
    endpoint: &ResolvedEndpoint,
) -> Result<SelectedAuth, CallError> {
    let components = &client.components;
    let options =
        components
            .auth_scheme_option_resolver
            .resolve(&AuthSchemeOptionResolverParams {
                model: &client.model,
                service: &client.service,
                operation,
                endpoint,
                signing_enabled: client.settings.signing.value,
                client_region: client.region.as_deref(),
                payload_signing: client
                    .settings
                    .payload_signing
                    .as_ref()
                    .map(|setting| setting.value),
            })?;
    auth::choose(options, components.auth_schemes)
}
