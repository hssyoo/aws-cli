//! Hook adapters for the characterization tests: customizations a frozen
//! harness gives a [`ClientCase`]'s client, each with a record of what it
//! saw, and a client builder that hands back its runtime, so a harness can
//! read the sleeps a call asked for.
//!
//! Like the rest of the façade, these produce data and decide nothing:
//! each adapter records what its position was given, and the harness compares.
//! Their signatures are frozen with the characterization tests; when the
//! call loop's stages change (interceptors, orchestrator), the bodies here
//! follow them.
use super::{CallError, Client, ClientCase, Customization, FixedRuntime};
use crate::endpoint::{EndpointConfig, EndpointInputs};
use crate::hooks::{HookError, HookResult, Order, Scope};
use crate::http::{HttpRequest, HttpResponse};
use crate::interceptors::{Intercept, InterceptorContext};
use crate::retry::{Attempt, RetryDecision};
use crate::runtime_components::ClassifyRetry;
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
fn table(id: &'static str, interceptor: Box<dyn Intercept + Send>) -> Table {
    let interceptor: &'static dyn Intercept = Box::leak(interceptor);
    let interceptors: &'static [&'static dyn Intercept] = Box::leak(Box::new([interceptor]));
    leak(Customization {
        interceptors,
        ..customization(id, Scope::ALL)
    })
}

/// A table of one customization of `scope` holding `classifier`.
fn classifier_table(
    id: &'static str,
    scope: Scope,
    classifier: Box<dyn ClassifyRetry + Send>,
) -> Table {
    let classifier: &'static dyn ClassifyRetry = Box::leak(classifier);
    let retry_classifiers: &'static [&'static dyn ClassifyRetry] =
        Box::leak(Box::new([classifier]));
    leak(Customization {
        retry_classifiers,
        ..customization(id, scope)
    })
}

/// A customization of `scope` that adds nothing.
const fn customization(id: &'static str, scope: Scope) -> Customization {
    Customization {
        id,
        scope,
        setting_defaults: &[],
        builtins: &[],
        interceptors: &[],
        retry_classifiers: &[],
        long_polling: false,
        replaces: &[],
    }
}

/// A table of `customization` alone.
fn leak(customization: Customization) -> Table {
    let customization: &'static Customization = Box::leak(Box::new(customization));
    Box::leak(Box::new([customization]))
}

/// `tables`' customizations, in order, as one table.
pub fn combined(tables: &[Table]) -> Table {
    let all: Vec<&'static Customization> = tables.iter().flat_map(|t| t.iter().copied()).collect();
    Box::leak(all.into_boxed_slice())
}

#[derive(Debug)]
struct LastModifyInput(Seen<Value>);

impl Intercept for LastModifyInput {
    fn name(&self) -> &'static str {
        "characterization-last-modify-input"
    }
    fn order(&self) -> Order {
        Order::Last
    }
    fn modify_before_serialization(
        &self,
        _: &mut InterceptorContext<'_>,
        input: &mut Value,
    ) -> HookResult {
        self.0.push(input.clone());
        Ok(())
    }
}

/// An [`Order::Last`] `modify_before_serialization` hook (`register_last`
/// on `before-parameter-build`), which records the input it is given.
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

impl Intercept for BeforeSign {
    fn name(&self) -> &'static str {
        "characterization-before-sign"
    }
    fn modify_before_signing(
        &self,
        ctx: &mut InterceptorContext<'_>,
        _: &mut HttpRequest,
    ) -> HookResult {
        self.seen.push(ctx.attempt);
        self.fails
            .as_ref()
            .map_or(Ok(()), |message| Err(HookError::Failed(message.clone())))
    }
}

/// A `modify_before_signing` hook (`before-sign`), which records each
/// attempt's number (1 for the first) and lets it go on.
pub fn before_sign_attempts() -> (Table, Seen<u32>) {
    let seen = Seen::new();
    let hook = BeforeSign {
        seen: seen.clone(),
        fails: None,
    };
    (table("characterization-before-sign", Box::new(hook)), seen)
}

/// A `modify_before_signing` hook that records each attempt's number,
/// then raises with `message` (a handler's exception, [`HookError::Failed`]).
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

impl Intercept for FailingShortCircuit {
    fn name(&self) -> &'static str {
        "characterization-failing-short-circuit"
    }
    fn short_circuit_transmit(
        &self,
        ctx: &mut InterceptorContext<'_>,
        _: &HttpRequest,
    ) -> Result<Option<HttpResponse>, HookError> {
        self.seen.push(ctx.attempt);
        Err(HookError::Failed(self.message.clone()))
    }
}

/// A `short_circuit_transmit` hook (`before-send`, the position that may
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

impl ClassifyRetry for DeferringClassifier {
    fn name(&self) -> &'static str {
        "characterization-deferring-classifier"
    }
    fn classify_retry(
        &self,
        ctx: &mut InterceptorContext<'_>,
        _: &Attempt<'_>,
    ) -> Option<RetryDecision> {
        self.0.push(ctx.attempt);
        None
    }
}

/// A retry classifier (a service's `needs-retry`
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
    let classifier = Box::new(DeferringClassifier(seen.clone()));
    (
        classifier_table("characterization-deferring-classifier", scope, classifier),
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
