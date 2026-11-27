//! How a call is authenticated, chosen as botocore chooses it
//! (`BaseClient._make_api_call`, `_resolve_endpoint_ruleset`,
//! `RequestSigner._choose_signer`, `set_operation_specific_signer`), and
//! SigV4 signing over the `sigv4` crate. SigV4 and none are implemented;
//! every other auth type is an admission refusal.
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
    AUTH_SCHEMES, NO_AUTH, NoAuthScheme, NoAuthSigner, SIGV4, SigV4AuthScheme, SigV4Signer,
};

/// An auth scheme's Smithy shape id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthSchemeId(&'static str);

impl AuthSchemeId {
    /// `aws.auth#sigv4`.
    pub const SIGV4: Self = Self("aws.auth#sigv4");
    /// `smithy.api#noAuth`.
    pub const NO_AUTH: Self = Self("smithy.api#noAuth");

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
        let option = select(
            p.model,
            p.service,
            p.operation,
            p.endpoint,
            p.signing_enabled,
            p.client_region,
        )?;
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

/// What a client signs with unless an operation or its endpoint says
/// otherwise: `signatureVersion`, else the service's `auth` trait, else
/// SigV4; nothing for an unsigned client (`--no-sign-request`).
fn client_signature_version(model: &Model, signing: bool) -> Result<Option<String>, Unsupported> {
    if !signing {
        return Ok(None);
    }
    // `signatureVersion` is botocore metadata, outside the schemas.
    if let Some(version) = model.metadata().signature_version.as_deref() {
        return Ok(Some(version.to_owned()));
    }
    match model
        .schema()
        .service()
        .auth()
        .and_then(|mut traits| traits.next())
    {
        Some(first) => resolve_auth_type(first).map(Some),
        None => Ok(Some("v4".to_owned())),
    }
}

/// What the model alone says the client cannot sign: the operation's auth
/// type (`auth`, else `authtype`), else the service's, when it is neither
/// SigV4 nor none, or an unsigned payload under SigV4. An endpoint's
/// `authSchemes` can still override the operation's type at resolution
/// ([`select`]); refusing on the model's is the conservative reading.
pub fn static_refusal(model: &Model, operation: OperationSchema<'_>) -> Option<Unsupported> {
    let version = match operation_auth_type(operation) {
        Err(unsupported) => return Some(unsupported),
        Ok(Some(version)) => version,
        Ok(None) => match client_signature_version(model, true) {
            Err(unsupported) => return Some(unsupported),
            Ok(version) => version.unwrap_or_else(|| "v4".to_owned()),
        },
    };
    match version.as_str() {
        "none" => None,
        "v4" if operation.unsigned_payload() => Some(Unsupported::UnsignedPayload),
        "v4" => None,
        other => Some(Unsupported::Auth(other.to_owned())),
    }
}

/// The option of a call of `operation` that resolved to `endpoint`, and the
/// signing region and name it signs with.
///
/// The endpoint's `authSchemes`, when it has any, replace the operation's
/// auth type and give the signing region and name; an unsigned client then
/// signs with none. Otherwise the region is the client's and the name the
/// model's `signingName`, else its `endpointPrefix`, else the service.
///
/// # Errors
/// [`CallError::Unsupported`] for an auth type other than SigV4 and none,
/// or a payload the operation leaves unsigned; `UnknownSignatureVersionError`
/// for an endpoint with no scheme botocore signs with.
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
    let version = match auth_type {
        Some(auth_type) => Some(auth_type),
        None => client_signature_version(model, signing).map_err(CallError::Unsupported)?,
    };
    match version.as_deref() {
        None | Some("none") => return Ok(AuthSchemeOption::no_auth()),
        Some("v4") if operation.unsigned_payload() => {
            return Err(CallError::Unsupported(Unsupported::UnsignedPayload));
        }
        Some("v4") => {}
        Some(other) => return Err(CallError::Unsupported(Unsupported::Auth(other.to_owned()))),
    }
    let service_schema = model.schema().service();
    let region = context.as_ref().and_then(|context| context.region.clone());
    let region = region.or_else(|| client_region.map(str::to_owned));
    let name = context.and_then(|context| context.name).or_else(|| {
        let default = service_schema
            .sigv4_name()
            .or(service_schema.endpoint_prefix());
        Some(default.unwrap_or(service).to_owned())
    });
    match (region, name) {
        (Some(region), Some(name)) => Ok(AuthSchemeOption {
            scheme_id: AuthSchemeId::SIGV4,
            signing: SigningOverrides {
                region: Some(region),
                name: Some(name),
            },
        }),
        _ => Err(CallError::NoRegion),
    }
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
    let url = request.url.to_string();
    let host = credentials::Location::parse(&url, false).map_or_else(
        || request.url.authority.to_ascii_lowercase(),
        |location| location.host_header,
    );
    let base: Vec<(&str, &str)> = request.headers.iter().collect();
    let signed = sigv4::sign(&sigv4::SignedRequest {
        credentials: sigv4::Credentials {
            access_key_id: &credentials.access_key_id,
            secret_access_key: &credentials.secret_access_key,
            session_token: credentials.session_token.as_deref(),
        },
        scope: sigv4::Scope {
            region,
            service: name,
        },
        timestamp: now,
        method: request.method.as_str(),
        host: &host,
        path: &request.url.path,
        query: &request.url.query,
        headers: &base,
        payload: sigv4::Payload::Body(&request.body),
        content_sha256: false,
        path_encoding: sigv4::PathEncoding::Normalized,
    });
    let headers = Headers::from_pairs(signed.apply(&base));
    request.headers = headers;
}

#[cfg(test)]
mod tests;
