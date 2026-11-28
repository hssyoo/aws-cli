//! How a call is authenticated, chosen as botocore chooses it
//! (`BaseClient._make_api_call`, `_resolve_endpoint_ruleset`,
//! `RequestSigner._choose_signer`, `set_operation_specific_signer`), and
//! SigV4 signing over the `sigv4` crate. SigV4, S3's SigV4 variant
//! (`S3SigV4Auth`) and none are implemented; every other auth type is an
//! admission refusal.
//!
//! The SRA's auth components, as the orchestrator runs them: the client's
//! [`AuthSchemeOptionResolver`] gives the call's options in `prepare` (it
//! may refuse), the first option an [`AuthScheme`] of the client serves is
//! chosen, and on every attempt that scheme's identity resolver and
//! [`Sign`]er sign the request. A new scheme (S3's SigV4, unsigned body,
//! SigV4a, bearer) is one more [`AuthScheme`] in [`AUTH_SCHEMES`], with the
//! fields it reads added to [`SigningOverrides`].
use crate::admission::Unsupported;
use crate::credentials::SigningCredentials;
use crate::endpoint::ResolvedEndpoint;
use crate::error::CallError;
use crate::http::{Headers, HttpRequest};
use crate::identity::{Identity, ResolveIdentity};
use crate::runtime_components::RuntimeComponents;
use model::schema::OperationSchema;
use model::shapes::Model;
use timestamp::Timestamp;

mod schemes;

pub use schemes::{
    AUTH_SCHEMES, NO_AUTH, NoAuthScheme, NoAuthSigner, S3_SIGV4, S3SigV4AuthScheme, S3SigV4Signer,
    SIGV4, SigV4AuthScheme, SigV4Signer,
};

/// An auth scheme's Smithy shape id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthSchemeId(&'static str);

impl AuthSchemeId {
    /// `aws.auth#sigv4`.
    pub const SIGV4: Self = Self("aws.auth#sigv4");
    /// `smithy.api#noAuth`.
    pub const NO_AUTH: Self = Self("smithy.api#noAuth");
    /// botocore's `s3v4` signer: S3's variant of `aws.auth#sigv4`, which
    /// Smithy does not name apart, so the S3 scheme has an id of its own.
    pub const S3_SIGV4: Self = Self("s3v4");

    /// The shape id.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// What a scheme signs with that the option resolver decides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SigningOverrides {
    /// The signing region: the endpoint's, else the client's.
    pub region: Option<String>,
    /// The signing name: the endpoint's, else the model's.
    pub name: Option<String>,
    /// The endpoint scheme's `disableDoubleEncoding`, when it gives one.
    /// botocore's signers never read it: `S3SigV4Auth` never encodes the
    /// path again, whatever it says, and `SigV4Auth` always does.
    pub disable_double_encoding: Option<bool>,
    /// The `s3` block's `payload_signing_enabled`, when configured, which
    /// only S3's signer reads (`S3SigV4Auth._should_sha256_sign_payload`).
    pub payload_signing: Option<bool>,
}

/// One way a call may be authenticated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthSchemeOption {
    pub scheme_id: AuthSchemeId,
    pub signing: SigningOverrides,
}

impl AuthSchemeOption {
    /// Sent unsigned.
    pub fn no_auth() -> Self {
        Self {
            scheme_id: AuthSchemeId::NO_AUTH,
            signing: SigningOverrides::default(),
        }
    }
}

/// What the option resolver decides from.
#[derive(Clone, Copy, Debug)]
pub struct AuthSchemeOptionResolverParams<'a> {
    pub model: &'a Model,
    /// The model directory name.
    pub service: &'a str,
    pub operation: OperationSchema<'a>,
    pub endpoint: &'a ResolvedEndpoint,
    /// False for an unsigned client (`--no-sign-request`).
    pub signing_enabled: bool,
    pub client_region: Option<&'a str>,
    /// The `s3` block's `payload_signing_enabled`, when configured.
    pub payload_signing: Option<bool>,
}

/// The auth option resolver: botocore's choice ([`select`]). One
/// implementation, so a concrete type (decision D4).
#[derive(Clone, Copy, Debug, Default)]
pub struct AuthSchemeOptionResolver;

impl AuthSchemeOptionResolver {
    /// The call's options in order of preference: exactly one today,
    /// botocore's choice.
    ///
    /// # Errors
    /// [`select`]'s, which refuse the call before any credential is read.
    pub fn resolve(
        &self,
        p: &AuthSchemeOptionResolverParams<'_>,
    ) -> Result<Vec<AuthSchemeOption>, CallError> {
        let mut option = select(
            p.model,
            p.service,
            p.operation,
            p.endpoint,
            p.signing_enabled,
            p.client_region,
        )?;
        if option.scheme_id == AuthSchemeId::S3_SIGV4 {
            option.signing.payload_signing = p.payload_signing;
        }
        Ok(vec![option])
    }
}

/// A way of authenticating a request: the identity it signs as and how it
/// signs.
pub trait AuthScheme: Sync + std::fmt::Debug {
    fn scheme_id(&self) -> AuthSchemeId;
    /// Where the identity comes from: the client's resolver, or the
    /// scheme's own.
    fn identity_resolver<'a>(&self, rc: &'a RuntimeComponents) -> &'a dyn ResolveIdentity;
    fn signer(&self) -> &dyn Sign;
}

/// Signs a request as one scheme does.
pub trait Sign: Sync + std::fmt::Debug {
    /// Signs `request` as `identity`, with what `option` decided.
    ///
    /// # Errors
    /// An identity or option the scheme cannot sign with.
    fn sign_http_request(
        &self,
        request: &mut HttpRequest,
        identity: &Identity,
        option: &AuthSchemeOption,
        rc: &RuntimeComponents,
    ) -> Result<(), CallError>;
}

/// The option a call signs with and the scheme that serves it.
#[derive(Clone, Debug)]
pub struct SelectedAuth {
    pub option: AuthSchemeOption,
    pub scheme: &'static dyn AuthScheme,
}

/// The first of `options` one of `schemes` serves, as the SRA chooses.
///
/// # Errors
/// [`Unsupported::Auth`] when none is served, so the call is refused
/// before any credential is read.
pub fn choose(
    options: Vec<AuthSchemeOption>,
    schemes: &[&'static dyn AuthScheme],
) -> Result<SelectedAuth, CallError> {
    let mut first = None;
    for option in options {
        if let Some(scheme) = schemes
            .iter()
            .find(|scheme| scheme.scheme_id() == option.scheme_id)
        {
            return Ok(SelectedAuth {
                option,
                scheme: *scheme,
            });
        }
        first.get_or_insert(option.scheme_id);
    }
    let named = first.map_or("no auth scheme", AuthSchemeId::as_str);
    Err(CallError::Unsupported(Unsupported::Auth(named.to_owned())))
}

/// `AUTH_TYPE_TO_SIGNATURE_VERSION`, for the auth types botocore signs with
/// (`resolve_auth_type`).
const AUTH_TRAITS: [(&str, &str); 4] = [
    ("aws.auth#sigv4", "v4"),
    ("aws.auth#sigv4a", "v4a"),
    ("smithy.api#httpBearerAuth", "bearer"),
    ("smithy.api#noAuth", "none"),
];

/// `OperationModel.resolved_auth_type`: the operation's `auth` trait,
/// resolved, else its `authtype`.
///
/// # Errors
/// An `auth` entry botocore does not know (`UnknownSignatureVersionError`).
fn operation_auth_type(operation: OperationSchema<'_>) -> Result<Option<String>, Unsupported> {
    match operation.auth().and_then(|mut traits| traits.next()) {
        Some(first) => resolve_auth_type(first).map(Some),
        None => Ok(operation.auth_type().map(str::to_owned)),
    }
}

/// `resolve_auth_type`: the first entry decides, as botocore returns or
/// raises on it.
fn resolve_auth_type(first: &str) -> Result<String, Unsupported> {
    AUTH_TRAITS
        .iter()
        .find(|(name, _)| *name == first)
        .map(|(_, version)| (*version).to_owned())
        .ok_or_else(|| Unsupported::Auth(first.to_owned()))
}

/// The signing names botocore signs with S3's signer (`S3_SIGNING_NAMES`).
const S3_SIGNING_NAMES: [&str; 4] = ["s3", "s3-outposts", "s3-object-lambda", "s3express"];

/// What a client signs with unless an operation or its endpoint says
/// otherwise (`ClientEndpointBridge._resolve_signature_version`): the
/// service's `auth` trait, else its `signatureVersion`, else SigV4; nothing
/// for an unsigned client (`--no-sign-request`). botocore also gives S3 by
/// name `s3v4`; every S3 and S3 Control endpoint names its scheme, so that
/// rule never picks the signer, and [`select`] refuses the one call it
/// could (SigV4 with an S3 signing name and no endpoint scheme).
fn client_signature_version(model: &Model, signing: bool) -> Result<Option<String>, Unsupported> {
    if !signing {
        return Ok(None);
    }
    if let Some(first) = model
        .schema()
        .service()
        .auth()
        .and_then(|mut traits| traits.next())
    {
        return resolve_auth_type(first).map(Some);
    }
    // `signatureVersion` is botocore metadata, outside the schemas.
    let version = model.metadata().signature_version.as_deref();
    Ok(Some(version.unwrap_or("v4").to_owned()))
}

/// The signers the client implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signer {
    None,
    SigV4,
    S3SigV4,
}

/// `set_operation_specific_signer`, for the auth type an operation or its
/// endpoint gives: SigV4 becomes S3's for an S3 signing name.
fn operation_signer(
    auth_type: &str,
    signing_name: &str,
    unsigned_payload: bool,
) -> Result<Signer, Unsupported> {
    match auth_type {
        "v4" if unsigned_payload => Err(Unsupported::UnsignedPayload),
        "v4" if S3_SIGNING_NAMES.contains(&signing_name) => Ok(Signer::S3SigV4),
        other => client_signer(other, unsigned_payload),
    }
}

/// The signer of a signature version no handler rewrites
/// (`AUTH_TYPE_MAPS`); an unsigned payload, which only a later handler
/// would honour, stays refused.
fn client_signer(version: &str, unsigned_payload: bool) -> Result<Signer, Unsupported> {
    match version {
        "none" => Ok(Signer::None),
        "v4" | "s3v4" if unsigned_payload => Err(Unsupported::UnsignedPayload),
        "v4" => Ok(Signer::SigV4),
        "s3v4" => Ok(Signer::S3SigV4),
        other => Err(Unsupported::Auth(other.to_owned())),
    }
}

/// The model's signing name: `signingName`, else `endpointPrefix`, else
/// the service.
fn model_signing_name(model: &Model, service: &str) -> String {
    let schema = model.schema().service();
    let name = schema.sigv4_name().or(schema.endpoint_prefix());
    name.unwrap_or(service).to_owned()
}

/// What the model alone says the client cannot sign: the operation's auth
/// type (`auth`, else `authtype`), else the client's, when it is neither
/// SigV4, S3's SigV4 nor none, or an unsigned payload under either SigV4.
/// An endpoint's `authSchemes` can still override the operation's type at
/// resolution ([`select`]); refusing on the model's is the conservative
/// reading.
pub fn static_refusal(
    model: &Model,
    service: &str,
    operation: OperationSchema<'_>,
) -> Option<Unsupported> {
    let unsigned = operation.unsigned_payload();
    let signer = match operation_auth_type(operation) {
        Err(unsupported) => return Some(unsupported),
        Ok(Some(auth_type)) => {
            operation_signer(&auth_type, &model_signing_name(model, service), unsigned)
        }
        Ok(None) => match client_signature_version(model, true) {
            Err(unsupported) => return Some(unsupported),
            Ok(version) => client_signer(version.as_deref().unwrap_or("v4"), unsigned),
        },
    };
    signer
        .and_then(|signer| streams_to_s3(signer, operation))
        .err()
}

/// A streaming input under S3's signer, which botocore sends unsigned over
/// https beside a checksum header (`S3SigV4Auth._should_sha256_sign_payload`
/// with `has_streaming_input`): the S3 signer signs every payload, so the
/// call is refused as streaming.
fn streams_to_s3(signer: Signer, operation: OperationSchema<'_>) -> Result<Signer, Unsupported> {
    if signer == Signer::S3SigV4 && crate::checksum::streaming_input(operation) {
        return Err(Unsupported::Trait("streaming".to_owned()));
    }
    Ok(signer)
}

/// The option of a call of `operation` that resolved to `endpoint`, and the
/// signing region and name it signs with.
///
/// The endpoint's `authSchemes`, when it has any, replace the operation's
/// auth type and give the signing region, name and `disableDoubleEncoding`;
/// an unsigned client then signs with none. That auth type, if any, picks
/// the signer as `set_operation_specific_signer` does (SigV4 is S3's for an
/// S3 signing name); else the client's signature version does. The region
/// is the endpoint's, else the client's; the name the endpoint's, else the
/// model's `signingName`, else its `endpointPrefix`, else the service.
///
/// # Errors
/// [`CallError::Unsupported`] for an auth type other than SigV4, S3's
/// SigV4 and none, or a payload the operation leaves unsigned;
/// `UnknownSignatureVersionError` for an endpoint with no scheme botocore
/// signs with.
pub fn select(
    model: &Model,
    service: &str,
    operation: OperationSchema<'_>,
    endpoint: &ResolvedEndpoint,
    signing: bool,
    client_region: Option<&str>,
) -> Result<AuthSchemeOption, CallError> {
    let mut auth_type = operation_auth_type(operation).map_err(CallError::Unsupported)?;
    let mut context = None;
    if !endpoint.auth_schemes.is_empty() {
        if signing {
            context = endpoint.signing_context().map_err(CallError::Endpoint)?;
            auth_type = context.as_ref().map(|context| context.auth_type.clone());
        } else {
            auth_type = Some("none".to_owned());
        }
    }
    let name = context
        .as_ref()
        .and_then(|context| context.name.clone())
        .unwrap_or_else(|| model_signing_name(model, service));
    let unsigned = operation.unsigned_payload();
    let signer = match auth_type {
        Some(auth_type) => operation_signer(&auth_type, &name, unsigned),
        None => match client_signature_version(model, signing) {
            // botocore signs this with S3's signer for S3's own model and
            // with SigV4 for any other, which only the service name tells.
            Ok(Some(version)) if version == "v4" && S3_SIGNING_NAMES.contains(&name.as_str()) => {
                Err(Unsupported::Auth(version))
            }
            Ok(Some(version)) => client_signer(&version, unsigned),
            Ok(None) => Ok(Signer::None),
            Err(unsupported) => Err(unsupported),
        },
    };
    let signer = signer.and_then(|signer| streams_to_s3(signer, operation));
    let scheme_id = match signer.map_err(CallError::Unsupported)? {
        Signer::None => return Ok(AuthSchemeOption::no_auth()),
        Signer::SigV4 => AuthSchemeId::SIGV4,
        Signer::S3SigV4 => AuthSchemeId::S3_SIGV4,
    };
    let region = context.as_ref().and_then(|context| context.region.clone());
    let Some(region) = region.or_else(|| client_region.map(str::to_owned)) else {
        return Err(CallError::NoRegion);
    };
    Ok(AuthSchemeOption {
        scheme_id,
        signing: SigningOverrides {
            region: Some(region),
            name: Some(name),
            disable_double_encoding: context.and_then(|context| context.disable_double_encoding),
            payload_signing: None,
        },
    })
}

/// Signs `request` as `SigV4Auth.add_auth` does: over the Host the
/// transport sends for its URL (both come from `credentials::Location`, so
/// the signed Host is the Host on the wire), the body's
/// SHA-256, and the session token, replacing any signing header the
/// request already carries.
pub fn sign(
    region: &str,
    name: &str,
    credentials: &SigningCredentials,
    now: Timestamp,
    request: &mut HttpRequest,
) {
    sign_as(
        Variant::SigV4,
        sigv4::Scope {
            region,
            service: name,
        },
        credentials,
        now,
        request,
    );
}

/// Signs `request` as `S3SigV4Auth.add_auth` does: as [`sign`], but over
/// the path as sent (`_normalize_url_path` returns it unchanged), with
/// `X-Amz-Content-SHA256` always sent, and the body's SHA-256 signed, or
/// `UNSIGNED-PAYLOAD` when `sign_payload` is false.
pub fn sign_s3(
    scope: sigv4::Scope<'_>,
    sign_payload: bool,
    credentials: &SigningCredentials,
    now: Timestamp,
    request: &mut HttpRequest,
) {
    sign_as(
        Variant::S3 { sign_payload },
        scope,
        credentials,
        now,
        request,
    );
}

/// Which of botocore's header signers [`sign_as`] signs as.
#[derive(Clone, Copy)]
enum Variant {
    SigV4,
    S3 { sign_payload: bool },
}

fn sign_as(
    variant: Variant,
    scope: sigv4::Scope<'_>,
    credentials: &SigningCredentials,
    now: Timestamp,
    request: &mut HttpRequest,
) {
    let url = request.url.to_string();
    let host = credentials::Location::parse(&url, false).map_or_else(
        || request.url.authority.to_ascii_lowercase(),
        |location| location.host_header,
    );
    let (payload, content_sha256, path_encoding) = match variant {
        Variant::SigV4 => (
            sigv4::Payload::Body(&request.body),
            false,
            sigv4::PathEncoding::Normalized,
        ),
        Variant::S3 { sign_payload } => (
            match sign_payload {
                true => sigv4::Payload::Body(&request.body),
                false => sigv4::Payload::Unsigned,
            },
            true,
            sigv4::PathEncoding::Raw,
        ),
    };
    let base: Vec<(&str, &str)> = request.headers.iter().collect();
    let signed = sigv4::sign(&sigv4::SignedRequest {
        credentials: sigv4::Credentials {
            access_key_id: &credentials.access_key_id,
            secret_access_key: &credentials.secret_access_key,
            session_token: credentials.session_token.as_deref(),
        },
        scope,
        timestamp: now,
        method: request.method.as_str(),
        host: &host,
        path: &request.url.path,
        query: &request.url.query,
        headers: &base,
        payload,
        content_sha256,
        path_encoding,
    });
    let headers = Headers::from_pairs(signed.apply(&base));
    request.headers = headers;
}

#[cfg(test)]
mod tests;
