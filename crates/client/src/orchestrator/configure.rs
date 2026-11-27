//! The client configuration phase: what `Client::new` builds once, so
//! that all of a client's calls and pages share it. botocore's retry
//! quota is per client (`RetryQuotaChecker`), so one built per call would
//! give every call a full quota.
use crate::call::{Client, ClientParts};
use crate::error::CallError;
use crate::hooks::{Defaults, Hooks};
use crate::protocol::ClientProtocol;
use crate::retry::{ErrorKind, Standard};
use crate::runtime_components::{EndpointResolver, RuntimeComponents};
use crate::runtime_plugin::ServicePlugins;
use crate::transport::Timeouts;
use std::time::Duration;

/// botocore's standard mode default (`DEFAULT_MAX_ATTEMPTS`).
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// A client of `parts` speaking `protocol`: its customizations chosen by
/// scope, its one retry strategy, its region and timeouts.
///
/// # Errors
/// `NoRegionError`, which botocore raises as it creates the client.
pub(crate) fn configure(
    parts: ClientParts,
    protocol: &'static dyn ClientProtocol,
) -> Result<Client, CallError> {
    let hooks = Hooks::for_service(parts.customizations, &parts.service);
    let configured = parts
        .settings
        .max_attempts
        .as_ref()
        .map(|attempts| u32::try_from(attempts.value).unwrap_or(u32::MAX));
    let retry_strategy = retry_strategy(hooks.defaults(), configured);
    let region = parts.endpoint.client_region(&parts.endpoints)?;
    let timeouts = Timeouts {
        connect: timeout(parts.settings.connect_timeout_ms.value),
        read: timeout(parts.settings.read_timeout_ms.value),
    };
    let components = RuntimeComponents {
        protocol,
        endpoint_resolver: EndpointResolver {
            data: parts.endpoints,
            config: parts.endpoint,
        },
        retry_strategy,
        identity_resolver: parts.credentials,
        http_client: parts.transport,
        runtime: parts.runtime,
        // Today's customizations are `hooks`; they move here when they
        // become plugins (S10).
        plugins: ServicePlugins::for_service(&[], &[], &parts.service),
    };
    Ok(Client {
        service: parts.service,
        model: parts.model,
        settings: parts.settings,
        customizations: parts.customizations,
        parse: parts.parse,
        user_agent: parts.user_agent,
        components,
        hooks,
        region,
        timeouts,
        http_checksum: false,
    })
}

/// The standard strategy as `ClientArgsCreator` configures it: the user's
/// `max_attempts`, else a customization's default, else 3; a
/// customization's base delay for errors that are not throttling, else
/// the standard one.
fn retry_strategy(defaults: &Defaults, configured: Option<u32>) -> Standard {
    let max_attempts = defaults
        .max_attempts(configured)
        .unwrap_or(DEFAULT_MAX_ATTEMPTS);
    let base = defaults.retry_base_delay();
    Standard::new(max_attempts, base.unwrap_or(ErrorKind::Transient.base()))
}

/// A timeout in milliseconds as the transport takes it: 0, botocore's
/// "none", is the longest the transport can wait.
fn timeout(milliseconds: u32) -> Duration {
    match milliseconds {
        0 => Duration::from_millis(u64::from(u32::MAX)),
        ms => Duration::from_millis(u64::from(ms)),
    }
}
