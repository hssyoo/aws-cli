use super::*;
use crate::interceptors::{InterceptorContext, Order};
use crate::retry::{Attempt, RetryDecision};
use std::time::Duration;

#[derive(Debug)]
struct Named(&'static str, Order);

impl Intercept for Named {
    fn name(&self) -> &'static str {
        self.0
    }
    fn order(&self) -> Order {
        self.1
    }
}

#[derive(Debug)]
struct Classifier(&'static str);

impl ClassifyRetry for Classifier {
    fn name(&self) -> &'static str {
        self.0
    }
    fn classify_retry(
        &self,
        _: &mut InterceptorContext<'_>,
        _: &Attempt<'_>,
    ) -> Option<RetryDecision> {
        None
    }
}

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

static CORE_FIRST: Named = Named("core-first", Order::First);
static CORE_LAST: Named = Named("core-last", Order::Last);
static SVC_DEFAULT: Named = Named("svc-default", Order::Default);
static SVC_FIRST: Named = Named("svc-first", Order::First);
static OP_DEFAULT: Named = Named("op-default", Order::Default);
static PLUGIN_FIRST: Named = Named("plugin-first", Order::First);
static PLUGIN_DEFAULT: Named = Named("plugin-default", Order::Default);
static PLUGIN_LAST: Named = Named("plugin-last", Order::Last);
static OTHER: Named = Named("other", Order::First);

static CORE: Customization = Customization {
    interceptors: &[&CORE_LAST, &CORE_FIRST],
    retry_classifiers: &[&Classifier("core")],
    setting_defaults: &[SettingDefault::MaxAttempts(4)],
    ..customization("core", Scope::ALL)
};
static SVC: Customization = Customization {
    interceptors: &[&SVC_DEFAULT, &SVC_FIRST],
    retry_classifiers: &[&Classifier("svc")],
    setting_defaults: &[
        SettingDefault::MaxAttempts(9),
        SettingDefault::RetryBaseDelay(Duration::from_millis(25)),
    ],
    ..customization("svc", Scope::service("svc"))
};
static SVC_ONE: Customization = Customization {
    interceptors: &[&OP_DEFAULT],
    retry_classifiers: &[&Classifier("svc-one")],
    setting_defaults: &[SettingDefault::RetryBaseDelay(Duration::from_secs(9))],
    long_polling: true,
    ..customization("svc-one", Scope::operation("svc", "One"))
};
static ELSEWHERE: Customization = Customization {
    interceptors: &[&OTHER],
    retry_classifiers: &[&Classifier("elsewhere")],
    setting_defaults: &[SettingDefault::MaxAttempts(1)],
    long_polling: true,
    ..customization("elsewhere", Scope::service("other"))
};
static PLUGIN: Customization = Customization {
    interceptors: &[&PLUGIN_LAST, &PLUGIN_DEFAULT, &PLUGIN_FIRST],
    retry_classifiers: &[&Classifier("plugin")],
    setting_defaults: &[
        SettingDefault::MaxAttempts(2),
        SettingDefault::RetryBaseDelay(Duration::from_secs(1)),
    ],
    ..customization("plugin", Scope::service("svc"))
};

static BUILT_INS: [&Customization; 4] = [&CORE, &SVC, &SVC_ONE, &ELSEWHERE];

fn names(plugins: &ServicePlugins, operation: &str) -> String {
    let names: Vec<_> = plugins
        .interceptors(operation)
        .map(Intercept::name)
        .collect();
    names.join(" ")
}

fn classifiers(plugins: &ServicePlugins, operation: &str) -> String {
    let names: Vec<_> = plugins
        .retry_classifiers(operation)
        .map(ClassifyRetry::name)
        .collect();
    names.join(" ")
}

#[test]
fn interceptors_run_by_band_then_origin_then_registration_order() {
    let built_in = ServicePlugins::for_service(&BUILT_INS, &[], "svc");
    let expected = "core-first svc-first svc-default op-default core-last";
    assert_eq!(names(&built_in, "One"), expected);
    let expected = "core-first svc-first svc-default core-last";
    assert_eq!(names(&built_in, "Two"), expected, "op scope filters");
    let both = ServicePlugins::for_service(&BUILT_INS, &[&PLUGIN], "svc");
    let expected = concat!(
        "core-first svc-first plugin-first svc-default op-default plugin-default ",
        "core-last plugin-last"
    );
    assert_eq!(
        names(&both, "One"),
        expected,
        "no plugin before a built-in of its band"
    );
    let configured = ServicePlugins::for_service(&[], &[&PLUGIN, &SVC], "svc");
    let expected = "plugin-first svc-first plugin-default svc-default plugin-last";
    assert_eq!(names(&configured, "Two"), expected, "configured order");
}

#[test]
fn classifiers_and_long_polling_are_filtered_by_scope() {
    let plugins = ServicePlugins::for_service(&BUILT_INS, &[&PLUGIN], "svc");
    assert_eq!(classifiers(&plugins, "One"), "core svc svc-one plugin");
    assert_eq!(classifiers(&plugins, "Two"), "core svc plugin");
    assert!(plugins.long_polls("One"));
    assert!(!plugins.long_polls("Two"));
    let other = ServicePlugins::for_service(&BUILT_INS, &[], "other");
    assert!(other.long_polls("Anything"));
    let none = ServicePlugins::for_service(&BUILT_INS, &[], "zzz");
    assert_eq!(names(&none, "One"), "core-first core-last");
    assert_eq!(classifiers(&none, "One"), "core");
    assert!(!none.long_polls("One"));
}

#[test]
fn the_first_service_wide_default_wins_built_ins_first() {
    let defaults = *ServicePlugins::for_service(&BUILT_INS, &[&PLUGIN], "svc").defaults();
    assert_eq!(defaults.max_attempts(None), Some(4), "CORE comes first");
    assert_eq!(defaults.retry_base_delay(), Some(Duration::from_millis(25)));
    let op_scoped = ServicePlugins::for_service(&[&SVC_ONE], &[], "svc");
    assert_eq!(
        *op_scoped.defaults(),
        SettingDefaults::default(),
        "op scope supplies none"
    );
    let over = ServicePlugins::for_service(&[&SVC], &[&PLUGIN], "svc");
    assert_eq!(over.defaults().max_attempts(None), Some(9));
    let only = ServicePlugins::for_service(&[], &[&PLUGIN], "svc");
    assert_eq!(only.defaults().max_attempts(None), Some(2));
    assert_eq!(
        only.defaults().retry_base_delay(),
        Some(Duration::from_secs(1))
    );
}
