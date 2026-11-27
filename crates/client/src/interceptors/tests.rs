use super::*;
use crate::http::{Headers, Method, Url};
use model::schema::{OperationData, OperationShapes, SchemaSet, SchemaSetBuilder};
use std::sync::Mutex;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A set with one operation, `Ping`, that takes and returns nothing.
fn schemas() -> TestResult<SchemaSet> {
    let mut builder = SchemaSetBuilder::new("c2j.svc")?;
    let unit = builder.prelude("Unit");
    let ping = OperationData {
        name: "Ping".into(),
        ..OperationData::default()
    };
    let shapes = OperationShapes {
        input: unit,
        output: unit,
        errors: Vec::new(),
    };
    builder.add_operation(ping, shapes)?;
    Ok(builder.finish()?)
}

fn context(set: &SchemaSet) -> TestResult<InterceptorContext<'_>> {
    let ping = set.service().operation("Ping").ok_or("no Ping")?;
    Ok(InterceptorContext::new("svcdir", Some("svc-id"), ping))
}

fn request() -> TestResult<HttpRequest> {
    Ok(HttpRequest {
        method: Method::parse("POST").ok_or("method")?,
        url: Url::parse("https://svc.example.com/").ok_or("url")?,
        headers: Headers::default(),
        body: b"{}".to_vec(),
    })
}

/// Implements nothing but its name.
#[derive(Debug)]
struct Inert;

impl Intercept for Inert {
    fn name(&self) -> &'static str {
        "inert"
    }
}

#[test]
fn unimplemented_hooks_change_nothing() -> TestResult {
    let set = schemas()?;
    let mut ctx = context(&set)?;
    let mut input = Value::String("in".into());
    let mut request = request()?;
    let before = request.clone();
    let mut response = HttpResponse::default();
    let mut builtins = Builtins::default();
    let mut outcome: Result<Value, CallError> = Ok(Value::Null);
    assert_eq!(Inert.order(), Order::Default);
    Inert.modify_before_serialization(&mut ctx, &mut input)?;
    Inert.modify_endpoint_builtins(&mut ctx, &mut builtins)?;
    Inert.modify_before_retry_loop(&mut ctx, &mut request)?;
    Inert.modify_before_signing(&mut ctx, &mut request)?;
    Inert.modify_before_transmit(&mut ctx, &mut request)?;
    assert_eq!(Inert.short_circuit_transmit(&mut ctx, &request)?, None);
    Inert.modify_before_deserialization(&mut ctx, &mut response)?;
    Inert.modify_before_attempt_completion(&mut ctx, &mut outcome)?;
    Inert.modify_before_completion(&mut ctx, &mut outcome)?;
    Inert.observe(HookPoint::BeforeExecution { input: &input }, &ctx)?;
    assert_eq!(
        (input, request == before),
        (Value::String("in".into()), true)
    );
    assert_eq!((response, builtins), Default::default());
    assert_eq!(outcome, Ok(Value::Null));
    assert!(ctx.into_state().is_empty());
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
struct Tokens(Vec<&'static str>);

#[test]
fn state_survives_the_prepared_call() -> TestResult {
    let set = schemas()?;
    let mut ctx = context(&set)?;
    ctx.state::<Tokens>().0.push("prepare");
    let shown = concat!(
        r#"InterceptorContext { service: "svcdir", service_id: Some("svc-id"), "#,
        r#"operation: "Ping", attempt: 1, state: Slots(1) }"#,
    );
    assert_eq!(format!("{ctx:?}"), shown);
    ctx.attempt = 3;
    let state = ctx.into_state();
    let ping = set.service().operation("Ping").ok_or("no Ping")?;
    let mut resumed = InterceptorContext::resume("svcdir", None, ping, state);
    // A resumed call starts at its first attempt.
    assert_eq!((resumed.attempt, resumed.service_id), (1, None));
    assert_eq!(resumed.find_state(), Some(&Tokens(vec!["prepare"])));
    resumed.state::<Tokens>().0.push("send");
    assert_eq!(resumed.state::<Tokens>().0, ["prepare", "send"]);
    Ok(())
}

/// Records each point it sees, and fails at `fails_at`.
#[derive(Debug, Default)]
struct Recorder {
    name: &'static str,
    fails_at: &'static str,
    seen: Mutex<Vec<String>>,
}

impl Recorder {
    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

impl Intercept for Recorder {
    fn name(&self) -> &'static str {
        self.name
    }
    fn observe(&self, point: HookPoint<'_>, ctx: &InterceptorContext<'_>) -> HookResult {
        let detail = match point {
            HookPoint::BeforeTransmit { request } => request.url.to_string(),
            HookPoint::AfterTransmit { response } => response.status.to_string(),
            _ => String::new(),
        };
        let entry = format!("{}@{}:{detail}", point.name(), ctx.attempt);
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(entry);
        }
        if point.name() == self.fails_at {
            return Err(HookError::Failed(format!("{} broke", self.name)));
        }
        Ok(())
    }
}

#[test]
fn an_observer_error_is_reported_and_the_rest_still_observe() -> TestResult {
    let set = schemas()?;
    let mut ctx = context(&set)?;
    ctx.attempt = 2;
    let (name, fails_at) = ("first", "read_before_transmit");
    let first = Recorder {
        name,
        fails_at,
        ..Recorder::default()
    };
    let (name, fails_at) = ("second", "read_after_transmit");
    let second = Recorder {
        name,
        fails_at,
        ..Recorder::default()
    };
    let request = request()?;
    let response = HttpResponse {
        status: 503,
        ..HttpResponse::default()
    };
    let mut failures = Vec::new();
    let both = || [&first as &dyn Intercept, &second];
    let mut report = |failure| failures.push(failure);
    let sent = HookPoint::BeforeTransmit { request: &request };
    let received = HookPoint::AfterTransmit {
        response: &response,
    };
    observe_all(both(), sent, &ctx, &mut report);
    observe_all(both(), received, &ctx, &mut report);
    let url = "read_before_transmit@2:https://svc.example.com/";
    for recorder in [&first, &second] {
        assert_eq!(recorder.seen(), [url, "read_after_transmit@2:503"]);
    }
    let failures: Vec<_> = failures
        .iter()
        .map(|f| (f.interceptor, f.point, f.error.to_string()))
        .collect();
    let broke = |name: &str| format!("{name} broke");
    assert_eq!(
        failures,
        [
            ("first", "read_before_transmit", broke("first")),
            ("second", "read_after_transmit", broke("second")),
        ]
    );
    Ok(())
}

#[test]
fn points_are_named_by_their_sra_position() -> TestResult {
    let input = Value::Null;
    let request = request()?;
    let response = HttpResponse::default();
    let output: Result<Value, CallError> = Ok(Value::Null);
    let points = [
        HookPoint::BeforeExecution { input: &input },
        HookPoint::BeforeSerialization { input: &input },
        HookPoint::AfterSerialization { request: &request },
        HookPoint::BeforeAttempt { request: &request },
        HookPoint::BeforeTransmit { request: &request },
        HookPoint::AfterTransmit {
            response: &response,
        },
        HookPoint::AfterDeserialization { output: &output },
        HookPoint::AfterAttempt { output: &output },
        HookPoint::AfterExecution { output: &output },
    ];
    let names: Vec<&str> = points.iter().map(HookPoint::name).collect();
    let expected = concat!(
        "read_before_execution read_before_serialization read_after_serialization ",
        "read_before_attempt read_before_transmit read_after_transmit ",
        "read_after_deserialization read_after_attempt read_after_execution",
    );
    assert_eq!(names.join(" "), expected);
    // The signing points are reserved, never observable.
    assert!(!names.iter().any(|name| name.contains("signing")));
    Ok(())
}
