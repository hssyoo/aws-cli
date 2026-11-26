//! Hook adapters for the characterization tests: customizations a frozen
//! harness gives a [`ClientCase`]'s client, each with a record of what it
//! saw, and a client builder that hands back its runtime, so a harness can
//! read the sleeps a call asked for.
//!
//! Like the rest of the façade, these produce data and decide nothing:
//! each adapter records what its stage was given, and the harness compares.
//! Their signatures are frozen with the characterization tests; when the
//! call loop's stages change (interceptors, orchestrator), the bodies here
//! follow them.
use super::{CallError, Client, ClientCase, Customization, FixedRuntime};
use crate::endpoint::{EndpointConfig, EndpointInputs};
use crate::hooks::{CallContext, HookError, HookResult, Interceptor, Order, Scope};
use crate::http::{HttpRequest, HttpResponse};
use crate::retry::{Attempt, RetryDecision};
use crate::useragent::{SessionExtras, UserAgentContext};
use model::value::Value;
use std::sync::{Arc, Mutex, PoisonError};

/// The customizations a case's client runs, as [`ClientCase`] takes them.
pub type Table = &'static [&'static Customization];

/// What an adapter saw, one entry each time its stage ran, in order.
#[derive(Debug)]
pub struct Seen<T>(Arc<Mutex<Vec<T>>>);

impl<T> Clone for Seen<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Clone> Seen<T> {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    fn push(&self, item: T) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(item);
    }
    /// Every entry so far.
    pub fn entries(&self) -> Vec<T> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A table of one customization of every call holding `interceptor`.
fn table(id: &'static str, interceptor: Box<dyn Interceptor + Send>) -> Table {
    scoped_table(id, Scope::ALL, interceptor)
}

/// A table of one customization of `scope` holding `interceptor`.
fn scoped_table(id: &'static str, scope: Scope, interceptor: Box<dyn Interceptor + Send>) -> Table {
    let interceptor: &'static dyn Interceptor = Box::leak(interceptor);
    let interceptors: &'static [&'static dyn Interceptor] = Box::leak(Box::new([interceptor]));
    let customization: &'static Customization = Box::leak(Box::new(Customization {
        id,
        scope,
        setting_defaults: &[],
        builtins: &[],
        interceptors,
        replaces: &[],
    }));
    Box::leak(Box::new([customization]))
}

/// `tables`' customizations, in order, as one table.
pub fn combined(tables: &[Table]) -> Table {
    let all: Vec<&'static Customization> = tables.iter().flat_map(|t| t.iter().copied()).collect();
    Box::leak(all.into_boxed_slice())
}

#[derive(Debug)]
struct LastModifyInput(Seen<Value>);

impl Interceptor for LastModifyInput {
    fn order(&self) -> Order {
        Order::Last
    }
    fn modify_input(&self, _: &mut CallContext<'_>, input: &mut Value) -> HookResult {
        self.0.push(input.clone());
        Ok(())
    }
}

/// An [`Order::Last`] `modify_input` interceptor (`register_last` on
/// `before-parameter-build`), which records the input it is given.
pub fn last_modify_input() -> (Table, Seen<Value>) {
    let seen = Seen::new();
    let table = table(
        "characterization-last-modify-input",
        Box::new(LastModifyInput(seen.clone())),
    );
    (table, seen)
}

#[derive(Debug)]
struct BeforeSign {
    seen: Seen<u32>,
    fails: Option<String>,
}

impl Interceptor for BeforeSign {
    fn before_sign(&self, cx: &mut CallContext<'_>, _: &mut HttpRequest) -> HookResult {
        self.seen.push(cx.attempt);
        self.fails
            .as_ref()
            .map_or(Ok(()), |message| Err(HookError::Failed(message.clone())))
    }
}

/// A `before_sign` interceptor (`before-sign`), which records each
/// attempt's number (1 for the first) and lets it go on.
pub fn before_sign_attempts() -> (Table, Seen<u32>) {
    let seen = Seen::new();
    let hook = BeforeSign {
        seen: seen.clone(),
        fails: None,
    };
    (table("characterization-before-sign", Box::new(hook)), seen)
}

/// A `before_sign` interceptor that records each attempt's number, then
/// raises with `message` (a handler's exception, [`HookError::Failed`]).
pub fn failing_before_sign(message: &str) -> (Table, Seen<u32>) {
    let seen = Seen::new();
    let hook = BeforeSign {
        seen: seen.clone(),
        fails: Some(message.to_owned()),
    };
    (
        table("characterization-failing-before-sign", Box::new(hook)),
        seen,
    )
}

#[derive(Debug)]
struct FailingShortCircuit {
    seen: Seen<u32>,
    message: String,
}

impl Interceptor for FailingShortCircuit {
    fn before_transmit(
        &self,
        cx: &mut CallContext<'_>,
        _: &HttpRequest,
    ) -> Result<Option<HttpResponse>, HookError> {
        self.seen.push(cx.attempt);
        Err(HookError::Failed(self.message.clone()))
    }
}

/// A `before_transmit` interceptor (`before-send`, the stage that may
/// answer in place of sending) that records each attempt's number, then
/// raises with `message`.
pub fn failing_short_circuit(message: &str) -> (Table, Seen<u32>) {
    let seen = Seen::new();
    let hook = FailingShortCircuit {
        seen: seen.clone(),
        message: message.to_owned(),
    };
    (
        table("characterization-failing-short-circuit", Box::new(hook)),
        seen,
    )
}

#[derive(Debug)]
struct DeferringClassifier(Seen<u32>);

impl Interceptor for DeferringClassifier {
    fn classify_retry(&self, cx: &mut CallContext<'_>, _: &Attempt<'_>) -> Option<RetryDecision> {
        self.0.push(cx.attempt);
        None
    }
}

/// A retry classifier (`classify_retry`, a service's `needs-retry`
/// handler such as DynamoDB's checksum check) that records each attempt it
/// is asked about and leaves the decision to standard mode. It applies to
/// `operation` of `service`, or to every operation of `service` for
/// `None`.
pub fn deferring_classifier(
    service: &'static str,
    operation: Option<&'static str>,
) -> (Table, Seen<u32>) {
    let seen = Seen::new();
    let scope = match operation {
        Some(operation) => Scope::operation(service, operation),
        None => Scope::service(service),
    };
    let hook = Box::new(DeferringClassifier(seen.clone()));
    (
        scoped_table("characterization-deferring-classifier", scope, hook),
        seen,
    )
}

/// [`super::client_case`]'s client, with the runtime it was built with,
/// whose [`FixedRuntime::slept`] names every sleep its calls asked for.
///
/// # Errors
/// What `Client::new` refuses.
pub fn client_with_runtime(case: ClientCase) -> Result<(Client, Arc<FixedRuntime>), CallError> {
    let runtime = Arc::new(FixedRuntime::new(case.now, case.random));
    let endpoint = EndpointConfig::new(&EndpointInputs {
        region: case.endpoint.region.as_deref(),
        use_fips: case.endpoint.use_fips,
        use_dual_stack: case.endpoint.use_dual_stack,
        endpoint_url: case.endpoint.endpoint_url.as_deref(),
        signing: true,
    });
    let user_agent = case.user_agent;
    let client = Client::new(crate::call::ClientParts {
        service: case.service,
        model: case.model,
        endpoints: case.endpoints,
        endpoint,
        settings: case.settings,
        credentials: Arc::new(case.credentials),
        transport: case.transport,
        runtime: runtime.clone(),
        customizations: case.customizations,
        parse: crate::protocol::ParseHooks::botocore(),
        user_agent: UserAgentContext {
            host: user_agent.host,
            distribution: user_agent.distribution,
            versions: user_agent.versions,
            extras: SessionExtras {
                installer: user_agent.installer,
                prompt: user_agent.prompt,
                command: user_agent.command,
            },
        },
    })?;
    Ok((client, runtime))
}
