use super::*;
use crate::endpoint::Builtins;
use crate::error::CallError;
use crate::interceptors::{Intercept, InterceptorContext};
use crate::runtime_plugin::ServicePlugins;
use model::schema::OperationSchema;
use model::shapes::Model;
use model::value::Value;
use std::time::Duration;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A model with the operations `Any`, `GetThing` and `PutThing`.
fn fixture() -> TestResult<Model> {
    let json = br#"{"metadata": {}, "shapes": {}, "operations": {
        "Any": {"name": "Any", "http": {"method": "POST", "requestUri": "/"}},
        "GetThing": {"name": "GetThing", "http": {"method": "POST", "requestUri": "/"}},
        "PutThing": {"name": "PutThing", "http": {"method": "POST", "requestUri": "/"}}}}"#;
    Ok(Model::from_json(json)?)
}

/// `model`'s operation `name`.
fn operation<'m>(model: &'m Model, name: &str) -> TestResult<OperationSchema<'m>> {
    Ok(model
        .schema()
        .service()
        .operation(name)
        .ok_or(name.to_owned())?)
}

/// A hook that says who it is through `modify_before_serialization`,
/// which appends its name to the input string.
#[derive(Debug)]
struct Named(&'static str, Order);

impl Intercept for Named {
    fn name(&self) -> &'static str {
        self.0
    }
    fn order(&self) -> Order {
        self.1
    }
    fn modify_before_serialization(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        input: &mut Value,
    ) -> HookResult {
        if let Value::String(text) = input {
            text.push_str(self.0);
        }
        Ok(())
    }
}

static LAST_A: Named = Named("[last-a]", Order::Last);
static DEFAULT_B: Named = Named("[default-b]", Order::Default);
static FIRST_C: Named = Named("[first-c]", Order::First);
static DEFAULT_D: Named = Named("[default-d]", Order::Default);
static OTHER: Named = Named("[other]", Order::First);
static ONE_OPERATION: Named = Named("[one-operation]", Order::Default);

static EVERYWHERE: Customization = Customization {
    id: "everywhere",
    scope: Scope::ALL,
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[&LAST_A, &DEFAULT_B],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[(
        "awscli.example.everywhere",
        Basis::Implemented {
            at: "client::runtime_plugin::ServicePlugins::for_service",
        },
    )],
};
static ZQUERY: Customization = Customization {
    id: "zquery",
    scope: Scope::service("zquery"),
    setting_defaults: &[SettingDefault::MaxAttempts(4)],
    builtins: &[],
    interceptors: &[&FIRST_C, &DEFAULT_D],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[],
};
static ZQUERY_ONE: Customization = Customization {
    id: "zquery-one",
    scope: Scope::operation("zquery", "GetThing"),
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[&ONE_OPERATION],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[],
};
static ZJSON: Customization = Customization {
    id: "zjson",
    scope: Scope::service("zjson"),
    setting_defaults: &[
        SettingDefault::MaxAttempts(5),
        SettingDefault::RetryBaseDelay(Duration::from_millis(25)),
    ],
    builtins: &[],
    interceptors: &[&OTHER],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[],
};
static TABLE: [&Customization; 4] = [&EVERYWHERE, &ZQUERY, &ZQUERY_ONE, &ZJSON];

/// What a client of `service` takes from the built-in `table`.
fn plugins(table: &[&'static Customization], service: &str) -> ServicePlugins {
    ServicePlugins::for_service(table, &[], service)
}

/// The names of the hooks that run for `operation`, in order.
fn run_order(hooks: &ServicePlugins, operation: &str) -> TestResult<String> {
    let model = fixture()?;
    let operation = self::operation(&model, operation)?;
    let mut cx = InterceptorContext::new("zquery", None, operation);
    let mut input = Value::String(String::new());
    for interceptor in hooks.interceptors(operation.name()) {
        interceptor.modify_before_serialization(&mut cx, &mut input)?;
    }
    match input {
        Value::String(text) => Ok(text),
        other => Err(format!("{other:?}").into()),
    }
}

#[test]
fn scopes_match_their_service_and_operation() {
    assert!(Scope::ALL.matches("zquery", "GetThing"));
    assert!(Scope::service("zquery").matches("zquery", "GetThing"));
    assert!(!Scope::service("zquery").matches("zjson", "GetThing"));
    let one = Scope::operation("zquery", "GetThing");
    assert!(one.matches("zquery", "GetThing"));
    assert!(!one.matches("zquery", "PutThing"));
    assert!(!one.matches("zjson", "GetThing"));
    assert!(one.matches_service("zquery"));
    assert!(!one.matches_service("zjson"));
}

/// A client keeps the interceptors whose scope names its service, ordered
/// First, Default, Last and, within each, by their place in the table; a
/// call runs those whose scope also names its operation.
#[test]
fn interceptors_are_chosen_by_scope_and_ordered_by_stage_then_table() -> TestResult {
    let hooks = plugins(&TABLE, "zquery");
    assert_eq!(
        run_order(&hooks, "GetThing")?,
        "[first-c][default-b][default-d][one-operation][last-a]"
    );
    assert_eq!(
        run_order(&hooks, "PutThing")?,
        "[first-c][default-b][default-d][last-a]"
    );
    let other = plugins(&TABLE, "zjson");
    assert_eq!(run_order(&other, "GetThing")?, "[other][default-b][last-a]");
    let none = plugins(&TABLE, "unrelated");
    assert_eq!(run_order(&none, "GetThing")?, "[default-b][last-a]");
    Ok(())
}

/// A customization's default applies only where the user left the setting
/// unset; the user's value, any value, wins.
#[test]
fn setting_defaults_apply_only_to_unset_settings() {
    let zquery = plugins(&TABLE, "zquery");
    assert_eq!(zquery.defaults().max_attempts(None), Some(4));
    assert_eq!(zquery.defaults().max_attempts(Some(2)), Some(2));
    assert_eq!(zquery.defaults().max_attempts(Some(1)), Some(1));
    assert_eq!(zquery.defaults().retry_base_delay(), None);
    let zjson = plugins(&TABLE, "zjson");
    assert_eq!(zjson.defaults().max_attempts(None), Some(5));
    assert_eq!(
        zjson.defaults().retry_base_delay(),
        Some(Duration::from_millis(25))
    );
    let none = plugins(&TABLE, "unrelated");
    assert_eq!(none.defaults().max_attempts(None), None);
}

/// An operation-scoped customization defaults no client setting: settings
/// belong to the client, which serves every operation.
#[test]
fn only_service_wide_customizations_default_settings() {
    static ONE_OP_DEFAULT: Customization = Customization {
        id: "one-op-default",
        scope: Scope::operation("zquery", "GetThing"),
        setting_defaults: &[SettingDefault::MaxAttempts(9)],
        builtins: &[],
        interceptors: &[],
        retry_classifiers: &[],
        long_polling: false,
        replaces: &[],
    };
    let hooks = plugins(&[&ONE_OP_DEFAULT], "zquery");
    assert_eq!(hooks.defaults().max_attempts(None), None);
}

/// The first customization in the table to default a setting wins.
#[test]
fn the_first_default_in_the_table_wins() {
    static LATER: Customization = Customization {
        id: "later",
        scope: Scope::ALL,
        setting_defaults: &[SettingDefault::MaxAttempts(7)],
        builtins: &[],
        interceptors: &[],
        retry_classifiers: &[],
        long_polling: false,
        replaces: &[],
    };
    let hooks = plugins(&[&ZQUERY, &LATER], "zquery");
    assert_eq!(hooks.defaults().max_attempts(None), Some(4));
}

#[derive(Debug, Default, PartialEq)]
struct Counter(u32);

#[derive(Debug, Default, PartialEq)]
struct Label(String);

/// Hooks share state through slots keyed by type: each type gets its own
/// slot, made from its default on first use and kept for the call.
#[test]
fn context_slots_are_typed_and_kept_for_the_call() -> TestResult {
    let model = fixture()?;
    let mut cx = InterceptorContext::new("zquery", None, operation(&model, "Any")?);
    assert_eq!(cx.attempt, 1);
    assert_eq!(*cx.state::<Counter>(), Counter(0));
    cx.state::<Counter>().0 += 2;
    cx.state::<Label>().0.push_str("signed");
    cx.state::<Counter>().0 += 1;
    assert_eq!(*cx.state::<Counter>(), Counter(3));
    assert_eq!(cx.state::<Label>().0, "signed");
    assert_eq!(cx.service, "zquery");
    assert_eq!(cx.operation.name(), "Any");
    let debug = format!("{cx:?}");
    assert!(debug.contains("zquery") && debug.contains("Any"), "{debug}");
    Ok(())
}

/// Every hook does nothing by default: a customization implements only
/// the hooks it needs, and the short-circuiting one declines.
#[test]
fn every_stage_defaults_to_doing_nothing() -> TestResult {
    let model = fixture()?;
    let mut cx = InterceptorContext::new("zquery", None, operation(&model, "Any")?);
    let hook: &dyn Intercept = &DEFAULT_B;
    let mut request = crate::http::HttpRequest {
        method: crate::http::Method::Post,
        url: crate::http::Url::parse("https://zquery.example/").ok_or("url")?,
        headers: crate::http::Headers::new(),
        body: Vec::new(),
    };
    let before = request.clone();
    let mut params = Builtins::default();
    hook.modify_endpoint_builtins(&mut cx, &mut params)?;
    hook.modify_before_retry_loop(&mut cx, &mut request)?;
    hook.modify_before_signing(&mut cx, &mut request)?;
    hook.modify_before_transmit(&mut cx, &mut request)?;
    assert_eq!(hook.short_circuit_transmit(&mut cx, &request)?, None);
    let mut response = crate::http::HttpResponse::default();
    hook.modify_before_deserialization(&mut cx, &mut response)?;
    let mut output: Result<Value, CallError> = Ok(Value::Null);
    hook.modify_before_completion(&mut cx, &mut output)?;
    assert_eq!(request, before);
    assert_eq!(params, Builtins::default());
    assert_eq!(output, Ok(Value::Null));
    Ok(())
}

/// A `Basis::Implemented` path resolves only to an item the source
/// defines.
#[test]
fn an_implemented_basis_names_an_item_that_exists() -> std::io::Result<()> {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(crate::testing::defines(
        &workspace,
        "client::call::Client::prepare"
    )?);
    assert!(crate::testing::defines(&workspace, "client::hooks::Basis")?);
    assert!(!crate::testing::defines(
        &workspace,
        "client::call::Client::no_such_item"
    )?);
    assert!(!crate::testing::defines(&workspace, "nosuchcrate::thing")?);
    Ok(())
}
