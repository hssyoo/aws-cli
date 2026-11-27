use super::*;
use crate::credentials::{CredentialError, CredentialSource};
use crate::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use crate::runtime_components::ComponentParts;
use crate::runtime_plugin::ServicePlugins;
use crate::testing::{FixedRuntime, InMemoryTransport};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const RULES: &str = r#"{"version": "1.0", "parameters": {},
    "rules": [{"conditions": [], "endpoint": {"url": "https://svc.example.com"},
               "type": "endpoint"}]}"#;

const PARTITIONS: &str = r#"{"version": "1.1", "partitions": [{"id": "aws",
    "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$", "regions": {},
    "outputs": {"name": "aws", "dnsSuffix": "example.com", "dualStackDnsSuffix": "example.com",
        "supportsFIPS": true, "supportsDualStack": true, "implicitGlobalRegion": "us-east-1"}}]}"#;

const MODEL: &[u8] = br#"{"metadata": {"protocol": "json", "jsonVersion": "1.0",
        "endpointPrefix": "svc", "signingName": "svcsign"},
    "operations": {
        "Signed": {"name": "Signed", "http": {"method": "POST", "requestUri": "/"}},
        "Open": {"name": "Open", "http": {"method": "POST", "requestUri": "/"},
                 "auth": ["smithy.api#noAuth"]}},
    "shapes": {}}"#;

/// These keys, counting each load.
#[derive(Debug, Default)]
struct Counted(AtomicUsize);

impl CredentialSource for Counted {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Some(keys()))
    }
}

fn keys() -> SigningCredentials {
    SigningCredentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "SECRET".into(),
        session_token: Some("TOKEN".into()),
        account_id: None,
    }
}

/// 2015-08-30T12:36:00Z.
fn clock() -> Timestamp {
    Timestamp::from_unix_seconds(1_440_938_160).unwrap_or_else(Timestamp::now)
}

fn components(model: &Model, credentials: Arc<Counted>) -> TestResult<RuntimeComponents> {
    let protocol = crate::protocol::for_model(model).ok_or("no protocol")?;
    let endpoints = EndpointData::from_documents(
        RULES.as_bytes().to_vec(),
        PARTITIONS.as_bytes().to_vec(),
        None,
    )?;
    let parts = ComponentParts {
        protocol,
        endpoints,
        endpoint: EndpointConfig::new(&EndpointInputs {
            region: Some("us-west-2"),
            ..EndpointInputs::default()
        }),
        credentials,
        transport: Arc::new(InMemoryTransport::new()),
        runtime: Arc::new(FixedRuntime::new(clock(), 0.0)),
        max_attempts: None,
    };
    Ok(RuntimeComponents::new(
        parts,
        ServicePlugins::for_service(&[], &[], "svc"),
    ))
}

fn request() -> TestResult<HttpRequest> {
    Ok(HttpRequest {
        method: crate::http::Method::Post,
        url: crate::http::Url::parse("https://svc.example.com/").ok_or("bad url")?,
        headers: Headers::from_pairs([("X-Amz-Target", "Svc.Signed")]),
        body: b"{}".to_vec(),
    })
}

fn sigv4(region: Option<&str>, name: Option<&str>) -> AuthSchemeOption {
    AuthSchemeOption {
        scheme_id: AuthSchemeId::SIGV4,
        signing: SigningOverrides {
            region: region.map(str::to_owned),
            name: name.map(str::to_owned),
        },
    }
}

fn params<'a>(
    model: &'a Model,
    operation: &str,
    endpoint: &'a ResolvedEndpoint,
    signing_enabled: bool,
) -> TestResult<AuthSchemeOptionResolverParams<'a>> {
    let operation = model
        .schema()
        .service()
        .operation(operation)
        .ok_or("no operation")?;
    Ok(AuthSchemeOptionResolverParams {
        model,
        service: "svc",
        operation,
        endpoint,
        signing_enabled,
        client_region: Some("us-west-2"),
    })
}

#[test]
fn the_resolver_gives_botocore_s_one_choice() -> TestResult {
    let model = Model::from_json(MODEL)?;
    let endpoint = ResolvedEndpoint {
        url: "https://svc.example.com".into(),
        auth_schemes: Vec::new(),
    };
    let resolver = AuthSchemeOptionResolver;
    let signed = resolver.resolve(&params(&model, "Signed", &endpoint, true)?)?;
    assert_eq!(signed, vec![sigv4(Some("us-west-2"), Some("svcsign"))]);
    let open = resolver.resolve(&params(&model, "Open", &endpoint, true)?)?;
    assert_eq!(open, vec![AuthSchemeOption::no_auth()]);
    let unsigned = resolver.resolve(&params(&model, "Signed", &endpoint, false)?)?;
    assert_eq!(unsigned, vec![AuthSchemeOption::no_auth()]);
    Ok(())
}

#[test]
fn the_first_served_option_is_chosen() -> TestResult {
    let unknown = AuthSchemeOption {
        scheme_id: AuthSchemeId("aws.auth#sigv4a"),
        signing: SigningOverrides::default(),
    };
    let chosen = choose(
        vec![unknown.clone(), AuthSchemeOption::no_auth()],
        &AUTH_SCHEMES,
    )?;
    assert_eq!(chosen.option, AuthSchemeOption::no_auth());
    assert_eq!(chosen.scheme.scheme_id(), AuthSchemeId::NO_AUTH);
    let chosen = choose(
        vec![sigv4(None, None), AuthSchemeOption::no_auth()],
        &AUTH_SCHEMES,
    )?;
    assert_eq!(chosen.scheme.scheme_id(), AuthSchemeId::SIGV4);
    Ok(())
}

#[test]
fn an_option_no_scheme_serves_is_refused() -> TestResult {
    let unknown = AuthSchemeOption {
        scheme_id: AuthSchemeId("aws.auth#sigv4a"),
        signing: SigningOverrides::default(),
    };
    let error = choose(vec![unknown], &AUTH_SCHEMES).err().ok_or("chose")?;
    assert!(
        matches!(&error, CallError::Unsupported(Unsupported::Auth(id)) if id == "aws.auth#sigv4a"),
        "{error:?}"
    );
    let error = choose(Vec::new(), &[&SIGV4]).err().ok_or("chose")?;
    assert!(
        matches!(error, CallError::Unsupported(Unsupported::Auth(_))),
        "{error:?}"
    );
    let error = choose(vec![AuthSchemeOption::no_auth()], &[&SIGV4])
        .err()
        .ok_or("chose")?;
    assert!(
        matches!(error, CallError::Unsupported(Unsupported::Auth(_))),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn the_client_serves_sigv4_and_no_auth_once_each() {
    let ids: Vec<AuthSchemeId> = AUTH_SCHEMES
        .iter()
        .map(|scheme| scheme.scheme_id())
        .collect();
    assert_eq!(ids, [AuthSchemeId::SIGV4, AuthSchemeId::NO_AUTH]);
}

#[test]
fn sigv4_signs_as_the_signer_does_with_the_client_s_credentials() -> TestResult {
    let model = Model::from_json(MODEL)?;
    let source = Arc::new(Counted::default());
    let rc = components(&model, source.clone())?;
    let resolver = SIGV4.identity_resolver(&rc);
    let identity = resolver.resolve_identity()?.ok_or("no identity")?;
    assert_eq!(source.0.load(Ordering::SeqCst), 1);
    let option = sigv4(Some("us-west-2"), Some("svcsign"));
    let mut signed = request()?;
    SIGV4
        .signer()
        .sign_http_request(&mut signed, &identity, &option, &rc)?;
    let mut expected = request()?;
    sign("us-west-2", "svcsign", &keys(), clock(), &mut expected);
    assert_eq!(signed, expected);
    assert!(signed.headers.contains("Authorization"));
    Ok(())
}

#[test]
fn sigv4_refuses_an_anonymous_identity_or_a_missing_region() -> TestResult {
    let model = Model::from_json(MODEL)?;
    let rc = components(&model, Arc::new(Counted::default()))?;
    let option = sigv4(Some("us-west-2"), Some("svcsign"));
    let signer = SIGV4.signer();
    let error = signer
        .sign_http_request(&mut request()?, &Identity::Anonymous, &option, &rc)
        .err()
        .ok_or("signed")?;
    assert!(matches!(error, CallError::Internal(_)), "{error:?}");
    let identity = Identity::Credentials(keys());
    for option in [sigv4(None, Some("svcsign")), sigv4(Some("us-west-2"), None)] {
        let error = signer
            .sign_http_request(&mut request()?, &identity, &option, &rc)
            .err()
            .ok_or("signed")?;
        assert!(matches!(error, CallError::NoRegion), "{error:?}");
    }
    Ok(())
}

#[test]
fn no_auth_reads_no_credentials_and_leaves_the_request_as_it_is() -> TestResult {
    let model = Model::from_json(MODEL)?;
    let source = Arc::new(Counted::default());
    let rc = components(&model, source.clone())?;
    let resolver = NO_AUTH.identity_resolver(&rc);
    let identity = resolver.resolve_identity()?.ok_or("no identity")?;
    assert_eq!(identity, Identity::Anonymous);
    assert_eq!(resolver.method(), None);
    let mut sent = request()?;
    let option = AuthSchemeOption::no_auth();
    NO_AUTH
        .signer()
        .sign_http_request(&mut sent, &identity, &option, &rc)?;
    assert_eq!(sent, request()?);
    // Even given credentials, nothing is signed.
    let credentials = Identity::Credentials(keys());
    NO_AUTH
        .signer()
        .sign_http_request(&mut sent, &credentials, &option, &rc)?;
    assert_eq!(sent, request()?);
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    Ok(())
}
