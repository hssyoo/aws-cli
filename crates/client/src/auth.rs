//! How a call is authenticated, chosen as botocore chooses it
//! (`BaseClient._make_api_call`, `_resolve_endpoint_ruleset`,
//! `RequestSigner._choose_signer`, `set_operation_specific_signer`), and
//! SigV4 signing over the `sigv4` crate. SigV4 and none are implemented;
//! every other auth type is an admission refusal.
use crate::admission::Unsupported;
use crate::credentials::SigningCredentials;
use crate::endpoint::ResolvedEndpoint;
use crate::error::CallError;
use crate::http::{Headers, HttpRequest};
use model::schema::OperationSchema;
use model::shapes::Model;
use timestamp::Timestamp;

/// A request's auth scheme, with what signing needs from the endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthScheme {
    /// SigV4 (`v4`), for the signing region and service name.
    SigV4 {
        signing_region: String,
        signing_name: String,
    },
    /// Sent unsigned (`smithy.api#noAuth`, `--no-sign-request`).
    None,
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

/// The scheme of a call of `operation` that resolved to `endpoint`, and the
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
) -> Result<AuthScheme, CallError> {
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
        None | Some("none") => return Ok(AuthScheme::None),
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
        (Some(signing_region), Some(signing_name)) => Ok(AuthScheme::SigV4 {
            signing_region,
            signing_name,
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
