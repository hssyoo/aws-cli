//! The client configuration phase: what `Client::new` builds once, so
//! that all of a client's calls and pages share it. botocore's retry
//! quota is per client (`RetryQuotaChecker`), so one built per call would
//! give every call a full quota.
use crate::call::{Client, ClientParts};
use crate::error::CallError;
use crate::protocol::ClientProtocol;
use crate::runtime_components::{ComponentParts, RuntimeComponents};
use crate::runtime_plugin::ServicePlugins;
use crate::transport::Timeouts;
use std::time::Duration;

/// A client of `parts` speaking `protocol`: its customizations chosen by
/// scope, its one retry strategy, its region and timeouts.
///
/// # Errors
/// `NoRegionError`, which botocore raises as it creates the client.
pub(crate) fn configure(
    parts: ClientParts,
    protocol: &'static dyn ClientProtocol,
) -> Result<Client, CallError> {
    // The built-in table only: no configured plugin exists yet.
    let plugins = ServicePlugins::for_service(parts.customizations, &[], &parts.service);
    let max_attempts = parts
        .settings
        .max_attempts
        .as_ref()
        .map(|attempts| u32::try_from(attempts.value).unwrap_or(u32::MAX));
    let region = parts.endpoint.client_region(&parts.endpoints)?;
    let timeouts = Timeouts {
        connect: timeout(parts.settings.connect_timeout_ms.value),
        read: timeout(parts.settings.read_timeout_ms.value),
    };
    let service_id = parts.model.facts().hyphenized_service_id();
    let components = RuntimeComponents::new(
        ComponentParts {
            protocol,
            endpoints: parts.endpoints,
            endpoint: parts.endpoint,
            credentials: parts.credentials,
            transport: parts.transport,
            runtime: parts.runtime,
            max_attempts,
        },
        plugins,
    );
    Ok(Client {
        service: parts.service,
        model: parts.model,
        settings: parts.settings,
        customizations: parts.customizations,
        parse: parts.parse,
        user_agent: parts.user_agent,
        service_id,
        components,
        region,
        timeouts,
    })
}

/// A timeout in milliseconds as the transport takes it: 0, botocore's
/// "none", is the longest the transport can wait.
fn timeout(milliseconds: u32) -> Duration {
    match milliseconds {
        0 => Duration::from_millis(u64::from(u32::MAX)),
        ms => Duration::from_millis(u64::from(ms)),
    }
}
