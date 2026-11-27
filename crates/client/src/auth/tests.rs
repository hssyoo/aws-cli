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
            ..SigningOverrides::default()
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
        payload_signing: None,
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
fn the_client_serves_sigv4_s3_sigv4_and_no_auth_once_each() {
    let ids: Vec<AuthSchemeId> = AUTH_SCHEMES
        .iter()
        .map(|scheme| scheme.scheme_id())
        .collect();
    assert_eq!(
        ids,
        [
            AuthSchemeId::SIGV4,
            AuthSchemeId::S3_SIGV4,
            AuthSchemeId::NO_AUTH
        ]
    );
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

/// A model with `metadata`'s extra fields and one operation, `Op`, whose
/// own fields are `operation`.
fn s3_model(metadata: &str, operation: &str) -> TestResult<Model> {
    let json = format!(
        r#"{{"metadata": {{"protocol": "rest-xml", "endpointPrefix": "zs3"{metadata}}},
            "operations": {{"Op": {{"name": "Op",
                "http": {{"method": "PUT", "requestUri": "/"}}{operation}}}}},
            "shapes": {{}}}}"#
    );
    Ok(Model::from_json(json.as_bytes())?)
}

fn scheme(name: &str, signing_name: &str) -> crate::endpoint::SchemeProps {
    crate::endpoint::SchemeProps {
        name: name.to_owned(),
        signing_name: Some(signing_name.to_owned()),
        signing_region: Some("eu-west-1".to_owned()),
        signing_region_set: None,
        disable_double_encoding: Some(true),
    }
}

fn resolve_on(
    model: &Model,
    service: &str,
    schemes: Vec<crate::endpoint::SchemeProps>,
) -> Result<AuthSchemeOption, CallError> {
    let endpoint = ResolvedEndpoint {
        url: "https://zs3.example.com".into(),
        auth_schemes: schemes,
    };
    let operation = model.schema().service().operation("Op");
    let operation = operation.ok_or_else(|| CallError::Internal("no Op".into()))?;
    select(
        model,
        service,
        operation,
        &endpoint,
        true,
        Some("us-west-2"),
    )
}

fn option(id: AuthSchemeId, region: &str, name: &str, dde: Option<bool>) -> AuthSchemeOption {
    AuthSchemeOption {
        scheme_id: id,
        signing: SigningOverrides {
            region: Some(region.into()),
            name: Some(name.into()),
            disable_double_encoding: dde,
            payload_signing: None,
        },
    }
}

/// The refusal `result` is.
fn refused(result: Result<AuthSchemeOption, CallError>) -> TestResult<Unsupported> {
    match result {
        Err(CallError::Unsupported(unsupported)) => Ok(unsupported),
        other => Err(format!("not refused: {other:?}").into()),
    }
}

fn auth(name: &str) -> Unsupported {
    Unsupported::Auth(name.into())
}

/// The client's version: the `auth` trait ahead of `signatureVersion`
/// (`ClientCreator.create_client`), which alone names `s3v4`; an
/// endpoint's `sigv4` with an S3 signing name is S3's signer
/// (`set_operation_specific_signer`), another name SigV4's.
#[test]
fn s3_s_signer_is_chosen_as_botocore_chooses_it() -> TestResult {
    let s3v4 = s3_model(r#", "signatureVersion": "s3v4""#, "")?;
    let plain = option(AuthSchemeId::S3_SIGV4, "us-west-2", "zs3", None);
    assert_eq!(resolve_on(&s3v4, "zs3", Vec::new())?, plain);
    let s3 = s3_model(r#", "signatureVersion": "s3""#, "")?;
    assert_eq!(refused(resolve_on(&s3, "zs3", Vec::new()))?, auth("s3"));
    // SigV4 for an S3 signing name, no endpoint scheme: S3's own model
    // would sign as S3, another as SigV4, so it is refused.
    let named = s3_model(r#", "auth": ["aws.auth#sigv4"], "signingName": "s3""#, "")?;
    assert_eq!(refused(resolve_on(&named, "zs3", Vec::new()))?, auth("v4"));
    let both = s3_model(
        r#", "signatureVersion": "s3", "auth": ["aws.auth#sigv4"]"#,
        "",
    )?;
    let v4 = option(AuthSchemeId::SIGV4, "us-west-2", "zs3", None);
    assert_eq!(resolve_on(&both, "zs3", Vec::new())?, v4);
    let ruled = [
        ("sigv4", "s3", AuthSchemeId::S3_SIGV4),
        ("sigv4", "s3-outposts", AuthSchemeId::S3_SIGV4),
        ("sigv4", "s3-object-lambda", AuthSchemeId::S3_SIGV4),
        ("sigv4", "s3express", AuthSchemeId::S3_SIGV4),
        ("sigv4", "s3control", AuthSchemeId::SIGV4),
        // A version the endpoint names as botocore spells it keeps its signer.
        ("s3v4", "zs3", AuthSchemeId::S3_SIGV4),
    ];
    for (scheme_name, name, id) in ruled {
        let expected = option(id, "eu-west-1", name, Some(true));
        let resolved = resolve_on(&both, "zs3", vec![scheme(scheme_name, name)])?;
        assert_eq!(resolved, expected, "{scheme_name} {name}");
    }
    Ok(())
}

/// S3 Express, SigV4a (MRAP) and an unsigned payload stay Python's, before
/// any credential; an auth trait ahead of `signatureVersion` refuses too.
#[test]
fn s3_s_unported_schemes_stay_refused() -> TestResult {
    let model = s3_model(r#", "signatureVersion": "s3""#, "")?;
    for (name, refusal) in [("sigv4-s3express", "v4-s3express"), ("sigv4a", "v4a")] {
        let result = resolve_on(&model, "s3", vec![scheme(name, "s3")]);
        assert_eq!(refused(result)?, auth(refusal), "{name}");
    }
    let unsigned = s3_model(
        r#", "signatureVersion": "s3v4""#,
        r#", "unsignedPayload": true"#,
    )?;
    for schemes in [Vec::new(), vec![scheme("sigv4", "s3")]] {
        let result = resolve_on(&unsigned, "zs3", schemes);
        assert_eq!(refused(result)?, Unsupported::UnsignedPayload);
    }
    let sigv4a = s3_model(
        r#", "signatureVersion": "v4", "auth": ["aws.auth#sigv4a", "aws.auth#sigv4"]"#,
        "",
    )?;
    assert_eq!(
        refused(resolve_on(&sigv4a, "zs3", Vec::new()))?,
        auth("v4a")
    );
    Ok(())
}

#[test]
fn the_static_refusal_reads_the_client_s_version_as_botocore_resolves_it() -> TestResult {
    let cases = [
        (r#", "signatureVersion": "s3""#, "", Some(auth("s3"))),
        (r#", "signatureVersion": "s3v4""#, "", None),
        (
            r#", "signatureVersion": "s3", "auth": ["aws.auth#sigv4"]"#,
            "",
            None,
        ),
        (
            r#", "signatureVersion": "v4", "auth": ["aws.auth#sigv4a"]"#,
            "",
            Some(auth("v4a")),
        ),
        (
            r#", "signatureVersion": "s3v4""#,
            r#", "unsignedPayload": true"#,
            Some(Unsupported::UnsignedPayload),
        ),
    ];
    for (metadata, operation, expected) in cases {
        let model = s3_model(metadata, operation)?;
        let op = model.schema().service().operation("Op").ok_or("no Op")?;
        assert_eq!(static_refusal(&model, "zs3", op), expected, "{metadata}");
    }
    Ok(())
}

/// A streaming input S3's signer would sign is refused as streaming,
/// statically and as the endpoint selects the signer: botocore leaves its
/// payload unsigned over https beside a checksum header.
#[test]
fn a_streaming_input_under_s3_s_signer_is_refused() -> TestResult {
    let model = |version: &str| -> TestResult<Model> {
        let json = format!(
            r#"{{"metadata": {{"protocol": "rest-xml", "endpointPrefix": "zs3",
                "signatureVersion": "{version}"}},
                "operations": {{"Op": {{"name": "Op", "http": {{"method": "PUT",
                    "requestUri": "/"}}, "input": {{"shape": "In"}}}}}},
                "shapes": {{"In": {{"type": "structure", "payload": "Body",
                    "members": {{"Body": {{"shape": "Stream"}}}}}},
                    "Stream": {{"type": "blob", "streaming": true}}}}}}"#
        );
        Ok(Model::from_json(json.as_bytes())?)
    };
    let streaming = Unsupported::Trait("streaming".into());
    let s3v4 = model("s3v4")?;
    let op = s3v4.schema().service().operation("Op").ok_or("no Op")?;
    assert_eq!(static_refusal(&s3v4, "zs3", op), Some(streaming.clone()));
    let plain = model("v4")?;
    let op = plain.schema().service().operation("Op").ok_or("no Op")?;
    assert_eq!(static_refusal(&plain, "zs3", op), None);
    let schemes = vec![scheme("sigv4", "s3")];
    assert_eq!(refused(resolve_on(&plain, "zs3", schemes))?, streaming);
    let schemes = vec![scheme("sigv4", "zs3")];
    assert!(resolve_on(&plain, "zs3", schemes).is_ok());
    Ok(())
}

/// The configured `payload_signing_enabled` reaches S3's option alone.
#[test]
fn only_s3_s_option_carries_the_payload_signing_setting() -> TestResult {
    let resolver = AuthSchemeOptionResolver;
    let endpoint = ResolvedEndpoint {
        url: "https://zs3.example.com".into(),
        auth_schemes: Vec::new(),
    };
    let s3v4 = s3_model(r#", "signatureVersion": "s3v4""#, "")?;
    let plain = Model::from_json(MODEL)?;
    for (model, operation, carried) in [(&s3v4, "Op", Some(false)), (&plain, "Signed", None)] {
        let mut p = params(model, operation, &endpoint, true)?;
        p.payload_signing = Some(false);
        let options = resolver.resolve(&p)?;
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].signing.payload_signing, carried, "{operation}");
    }
    Ok(())
}

/// A request `S3SigV4Auth.add_auth` signed, and what it set.
struct Vector {
    method: crate::http::Method,
    url: &'static str,
    body: &'static [u8],
    headers: &'static [(&'static str, &'static str)],
    payload_signing: Option<bool>,
    hash: &'static str,
    signature: &'static str,
}

const KEY_URL: &str = "https://bucket.s3.us-west-2.amazonaws.com/a%20b/~/%25/%C3%BC//x";
const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const TYPED: &[(&str, &str)] = &[("Content-Type", "text/plain")];

/// Computed by the vendored botocore's `S3SigV4Auth` at [`clock`] with
/// [`keys`], signing name `s3`, region `us-west-2`.
const VECTORS: [Vector; 5] = [
    Vector {
        method: crate::http::Method::Put,
        url: KEY_URL,
        body: b"foo bar baz",
        headers: TYPED,
        payload_signing: None,
        hash: "dbd318c1c462aee872f41109a4dfd3048871a03dedd0fe0e757ced57dad6f2d7",
        signature: "a91c66fc6070f8885f73451e6b25bf5b7f821a0e9ee125acc78a03b48ce57b28",
    },
    Vector {
        method: crate::http::Method::Put,
        url: KEY_URL,
        body: b"foo bar baz",
        headers: TYPED,
        payload_signing: Some(false),
        hash: "UNSIGNED-PAYLOAD",
        signature: "ada88d6a2ce1e489c798b630c3e3f20072e99d9e682971643cdcb9d84ad6e0bf",
    },
    Vector {
        method: crate::http::Method::Put,
        url: "http://bucket.s3.us-west-2.amazonaws.com/k",
        body: b"foo",
        headers: &[],
        payload_signing: Some(false),
        hash: "UNSIGNED-PAYLOAD",
        signature: "0b497b74d76bae4715cac1aff10ae6f0f2e6bdd01329a88d181a851cd9e8e35f",
    },
    Vector {
        method: crate::http::Method::Get,
        url: "https://s3.us-west-2.amazonaws.com/bucket?prefix=a%20b&list-type=2&marker=%C3%A4",
        body: b"",
        headers: &[],
        payload_signing: None,
        hash: EMPTY_HASH,
        signature: "a20c0f9bf448f2fc0991d49f97d434c8e9dbbb6357c7e874da71faf1c62f0351",
    },
    Vector {
        method: crate::http::Method::Get,
        url: "https://s3.amazonaws.com/bucket/foo/./bar/../bar",
        body: b"",
        headers: &[("x-amz-meta-a", "  b   c ")],
        payload_signing: None,
        hash: EMPTY_HASH,
        signature: "ce8b8206ee97f7db7ff7cb2de1d6444b15b86e5f70d262c356b6045405464752",
    },
];

/// Headers `S3SigV4Auth.add_auth` sets: the path as sent (no second
/// encoding, no dot segments removed), `X-Amz-Content-SHA256` always,
/// `UNSIGNED-PAYLOAD` when `payload_signing_enabled` is false over HTTPS or
/// plain HTTP, and the query sorted.
#[test]
fn s3_sigv4_signs_as_botocore_s_s3_signer() -> TestResult {
    let model = Model::from_json(MODEL)?;
    let rc = components(&model, Arc::new(Counted::default()))?;
    let identity = Identity::Credentials(keys());
    for vector in VECTORS {
        let mut request = HttpRequest {
            method: vector.method,
            url: crate::http::Url::parse(vector.url).ok_or("bad url")?,
            headers: Headers::from_pairs(vector.headers.iter().copied()),
            body: vector.body.to_vec(),
        };
        let mut option = option(AuthSchemeId::S3_SIGV4, "us-west-2", "s3", None);
        option.signing.payload_signing = vector.payload_signing;
        S3_SIGV4
            .signer()
            .sign_http_request(&mut request, &identity, &option, &rc)?;
        let mut names: Vec<&str> = vector.headers.iter().map(|(name, _)| *name).collect();
        names.extend(["X-Amz-Date", "X-Amz-Security-Token", "X-Amz-Content-SHA256"]);
        names.push("Authorization");
        let sent: Vec<&str> = request.headers.iter().map(|(name, _)| name).collect();
        assert_eq!(sent, names, "{}", vector.url);
        let hash = request.headers.get("X-Amz-Content-SHA256");
        assert_eq!(hash, Some(vector.hash), "{}", vector.url);
        let authorization = request.headers.get("Authorization").ok_or("unsigned")?;
        let signature = format!("Signature={}", vector.signature);
        assert!(authorization.ends_with(&signature), "{authorization}");
    }
    Ok(())
}

#[test]
fn s3_sigv4_refuses_an_anonymous_identity_or_a_missing_region() -> TestResult {
    let model = Model::from_json(MODEL)?;
    let rc = components(&model, Arc::new(Counted::default()))?;
    let signer = S3_SIGV4.signer();
    let option = option(AuthSchemeId::S3_SIGV4, "us-west-2", "s3", None);
    let error = signer
        .sign_http_request(&mut request()?, &Identity::Anonymous, &option, &rc)
        .err()
        .ok_or("signed")?;
    assert!(matches!(error, CallError::Internal(_)), "{error:?}");
    let mut regionless = option;
    regionless.signing.region = None;
    let identity = Identity::Credentials(keys());
    let error = signer
        .sign_http_request(&mut request()?, &identity, &regionless, &rc)
        .err()
        .ok_or("signed")?;
    assert!(matches!(error, CallError::NoRegion), "{error:?}");
    Ok(())
}
