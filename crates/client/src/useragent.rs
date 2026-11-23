//! The `User-Agent` a call sends (botocore's `UserAgentString.to_string`,
//! awscli's session extras): the client-wide parts from the settings, and
//! the feature ids the call itself registers, which [`render`] emits in
//! the order `cli_core` declares them (sorted, PLAN §2.1 Q3) where Python's
//! come in set order.
//!
//! [`render`]: cli_core::client_config::user_agent::render
use crate::endpoint::{EndpointParams, ParamValue};
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::{
    self, Distribution, Extras, Feature, Features, Host, Inputs, Versions,
};

/// The process facts the header states, read once by the caller
/// ([`user_agent::host`], [`user_agent::distribution`]).
#[derive(Clone, Debug)]
pub struct UserAgentContext {
    pub host: Host,
    pub distribution: Option<Distribution>,
    pub versions: Versions<'static>,
}

/// The feature ids a call registers as it resolves its endpoint
/// (`_register_endpoint_feature_ids`): the account id mode it passes, and
/// whether it passed an account id.
pub fn endpoint_features(params: &EndpointParams) -> Features {
    let mut features = Features::default();
    match params.get("AccountIdEndpointMode") {
        Some(ParamValue::String(mode)) if mode == "preferred" => {
            features.insert(Feature::AccountIdModePreferred);
        }
        Some(ParamValue::String(mode)) if mode == "disabled" => {
            features.insert(Feature::AccountIdModeDisabled);
        }
        _ => {}
    }
    if params.get("AccountId").is_some() {
        features.insert(Feature::ResolvedAccountId);
    }
    features
}

/// The header for a call of a client with `settings`: the settings' own
/// feature ids, its credential provider's, and `call`'s.
pub fn header(settings: &ClientSettings, context: &UserAgentContext, call: Features) -> String {
    let mut features = settings.user_agent_features();
    features.extend(settings.credential_features().iter().copied());
    features.extend(call.iter());
    let inputs = Inputs {
        execution_env: settings.execution_env.as_ref().map(|v| v.value.as_slice()),
        features,
        retry_mode: settings.retry_mode().value,
        app_id: settings.app_id.as_ref().map(|v| v.value.as_slice()),
        extras: Extras {
            installer: "other",
            prompt: "off",
            command: &settings.command,
        },
    };
    user_agent::render(
        &inputs,
        &context.host,
        context.distribution.as_ref(),
        context.versions,
    )
}
