use super::*;
use crate::config_bag::SettingDefault;
use crate::credentials::{CredentialError, SigningCredentials};
use crate::endpoint::{EndpointInputs, ParamValue};
use crate::runtime_plugin::{Customization, Scope};
use crate::testing::{FixedRuntime, InMemoryTransport};
use model::shapes::Model;
use std::time::Duration;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const RULES: &str = r#"{
    "version": "1.0",
    "parameters": {"Region": {"type": "String", "builtIn": "AWS::Region", "required": false}},
    "rules": [
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "Region"}]}],
         "endpoint": {"url": "https://svc.{Region}.example.com"}, "type": "endpoint"},
        {"conditions": [], "error": "Missing Region", "type": "error"}
    ]
}"#;

const PARTITIONS: &str = r#"{"version": "1.1", "partitions": [{"id": "aws",
    "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$", "regions": {},
    "outputs": {"name": "aws", "dnsSuffix": "example.com", "dualStackDnsSuffix": "example.com",
        "supportsFIPS": true, "supportsDualStack": true, "implicitGlobalRegion": "us-east-1"}}]}"#;

const MODEL: &[u8] = br#"{"metadata": {"protocol": "json", "jsonVersion": "1.0"},
    "operations": {}, "shapes": {}}"#;

#[derive(Debug)]
struct NoKeys;

impl CredentialSource for NoKeys {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        Ok(None)
    }
}

static TEN: Customization = Customization {
    id: "ten",
    scope: Scope::service("svc"),
    setting_defaults: &[
        SettingDefault::MaxAttempts(10),
        SettingDefault::RetryBaseDelay(Duration::from_millis(25)),
    ],
    builtins: &[],
    interceptors: &[],
    retry_classifiers: &[],
    long_polling: true,
    replaces: &[],
};

fn plugins(table: &[&'static Customization]) -> ServicePlugins {
    ServicePlugins::for_service(table, &[], "svc")
}

/// The first retry's delay at the top of its jitter, for a transient error.
fn first_transient_delay(strategy: &Standard) -> Option<Duration> {
    strategy
        .needs_retry(1, Some(ErrorKind::Transient), None, 1.0)
        .delay
}

#[test]
fn the_strategy_takes_the_users_then_a_plugins_settings() {
    assert_eq!(retry_strategy(&plugins(&[&TEN]), Some(2)).max_attempts(), 2);
    assert_eq!(retry_strategy(&plugins(&[&TEN]), None).max_attempts(), 10);
    assert_eq!(retry_strategy(&plugins(&[]), None).max_attempts(), 3);
    let standard = Standard::new(3, ErrorKind::Transient.base());
    let plain = retry_strategy(&plugins(&[]), None);
    assert_eq!(
        first_transient_delay(&plain),
        first_transient_delay(&standard)
    );
    let custom = retry_strategy(&plugins(&[&TEN]), None);
    let expected = Standard::new(10, Duration::from_millis(25));
    assert_eq!(
        first_transient_delay(&custom),
        first_transient_delay(&expected)
    );
    assert_ne!(
        first_transient_delay(&custom),
        first_transient_delay(&standard)
    );
}

#[test]
fn components_hold_what_the_client_was_built_from() -> TestResult {
    let model = Model::from_json(MODEL)?;
    let protocol = crate::protocol::for_model(&model).ok_or("no protocol")?;
    let endpoints = EndpointData::from_documents(
        RULES.as_bytes().to_vec(),
        PARTITIONS.as_bytes().to_vec(),
        None,
    )?;
    let transport = Arc::new(InMemoryTransport::new());
    let parts = ComponentParts {
        protocol,
        endpoints,
        endpoint: EndpointConfig::new(&EndpointInputs {
            region: Some("us-west-2"),
            ..EndpointInputs::default()
        }),
        credentials: Arc::new(NoKeys),
        transport: transport.clone(),
        runtime: Arc::new(FixedRuntime::new(timestamp::Timestamp::now(), 0.0)),
        max_attempts: None,
    };
    let components = RuntimeComponents::new(parts, plugins(&[&TEN]));
    assert_eq!(components.retry_strategy.max_attempts(), 10);
    assert!(components.plugins.long_polls("Any"));
    assert!(matches!(
        components.identity_resolver.resolve_identity(),
        Ok(None)
    ));
    assert!(Arc::ptr_eq(
        &components.http_client,
        &(transport as Arc<dyn Transport>)
    ));
    let mut params = EndpointParams::default();
    params.set("Region", ParamValue::String("eu-west-1".into()));
    let resolved = components.endpoint_resolver.resolve(&params)?;
    assert_eq!(resolved.url, "https://svc.eu-west-1.example.com");
    Ok(())
}
