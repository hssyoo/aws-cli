//! The `User-Agent` a call sends (botocore's `UserAgentString.to_string`,
//! awscli's session extras): the client-wide parts from the settings, and
//! the feature ids the call itself registers, which [`render`] emits in
//! the order `cli_core` declares them, so the header is the same on every
//! run; Python's come in set order (useragent.py
//! `UserAgentString._build_feature_metadata` joins a set), which varies.
//!
//! [`render`]: cli_core::client_config::user_agent::render
use crate::endpoint::{EndpointParams, ParamValue};
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::{
    self, Distribution, Extras, Feature, Features, Host, Inputs, Versions,
};

/// The process facts the header states, read once by the caller
/// ([`user_agent::host`], [`user_agent::distribution`]), and the extras the
/// caller's session appends after botocore's parts.
#[derive(Clone, Debug)]
pub struct UserAgentContext {
    pub host: Host,
    pub distribution: Option<Distribution>,
    pub versions: Versions<'static>,
    pub extras: SessionExtras,
}

/// What the AWS CLI's session adds to every `User-Agent`
/// (`awscli/clidriver.py`, `add_metadata_component_to_user_agent_extra`):
/// the install source, the auto-prompt mode and the command, which only
/// the CLI knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionExtras {
    /// `md/installer#<source>` (`get_distribution_source`).
    pub installer: &'static str,
    /// `md/prompt#<mode>`.
    pub prompt: &'static str,
    /// `md/command#<lineage>`: the command's names joined by dots.
    pub command: String,
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
/// feature ids and `call`'s, which hold the answering credential provider's
/// once the call has asked for credentials.
pub fn header(settings: &ClientSettings, context: &UserAgentContext, call: Features) -> String {
    let mut features = settings.user_agent_features();
    features.extend(call.iter());
    let inputs = Inputs {
        execution_env: settings.execution_env.as_ref().map(|v| v.value.as_slice()),
        features,
        retry_mode: settings.retry_mode().value,
        app_id: settings.app_id.as_ref().map(|v| v.value.as_slice()),
        extras: Extras {
            installer: context.extras.installer,
            prompt: context.extras.prompt,
            command: &context.extras.command,
        },
    };
    user_agent::render(
        &inputs,
        &context.host,
        context.distribution.as_ref(),
        context.versions,
    )
}
