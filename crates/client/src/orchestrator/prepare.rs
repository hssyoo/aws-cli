//! The prepare phases: everything that needs no credentials, network or
//! runtime, so every refusal comes before any credential provider runs.
use super::{find, prepare_hook_error};
use crate::admission::Unsupported;
use crate::auth::{self, AuthScheme};
use crate::call::{Client, Prepared};
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
    let (endpoint, mut features) = resolve_endpoint(client, &mut ctx, &input, None)?;
    features.extend(extra.iter());
    let serialized = serialize(client, operation, &input)?;
    let mut request = apply_endpoint(client, &serialized, &endpoint.url, features)?;
    modify_before_retry_loop(client, &mut ctx, &mut request)?;
    let scheme = resolve_auth_scheme(client, operation, &endpoint)?;
    Ok(Prepared {
        operation: operation.name().to_owned(),
        slots: ctx.into_state(),
        request,
        scheme,
        serialized,
        features,
        input,
        endpoint_url: endpoint.url,
    })
}

/// The static refusals ([`crate::admission::static_refusal`]), then what
/// botocore's client creation raises once a region resolves.
fn admit(client: &Client, operation: OperationSchema<'_>) -> Result<(), CallError> {
    if let Some(unsupported) = crate::admission::static_refusal(
        &client.model,
        operation,
        &client.components.endpoint_resolver.data,
        &client.service,
        client.customizations,
        // The paginator's entry was judged where the call was admitted.
        None,
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

/// The scheme botocore's selection gives (`choose-signer`), a component,
/// not a hook.
pub(super) fn resolve_auth_scheme(
    client: &Client,
    operation: OperationSchema<'_>,
    endpoint: &ResolvedEndpoint,
) -> Result<AuthScheme, CallError> {
    auth::select(
        &client.model,
        &client.service,
        operation,
        endpoint,
        client.settings.signing.value,
        client.region.as_deref(),
    )
}
