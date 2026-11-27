//! The auth schemes a client serves: SigV4 (`SigV4Auth`), S3's SigV4
//! (`S3SigV4Auth`) and none.
use super::{AuthScheme, AuthSchemeId, AuthSchemeOption, Sign};
use crate::credentials::SigningCredentials;
use crate::error::CallError;
use crate::http::HttpRequest;
use crate::identity::{AnonymousIdentityResolver, Identity, ResolveIdentity};
use crate::runtime_components::RuntimeComponents;

/// `aws.auth#sigv4`, signing as the client's credentials.
#[derive(Debug)]
pub struct SigV4AuthScheme {
    signer: SigV4Signer,
}

/// `SigV4Auth.add_auth`, over [`super::sign`].
#[derive(Debug)]
pub struct SigV4Signer;

/// botocore's `s3v4`, signing as the client's credentials.
#[derive(Debug)]
pub struct S3SigV4AuthScheme {
    signer: S3SigV4Signer,
}

/// `S3SigV4Auth.add_auth`, over [`super::sign_s3`].
#[derive(Debug)]
pub struct S3SigV4Signer;

/// `smithy.api#noAuth`: unsigned, reading no credentials.
#[derive(Debug)]
pub struct NoAuthScheme {
    signer: NoAuthSigner,
}

/// Leaves the request as it is.
#[derive(Debug)]
pub struct NoAuthSigner;

pub static SIGV4: SigV4AuthScheme = SigV4AuthScheme {
    signer: SigV4Signer,
};

pub static S3_SIGV4: S3SigV4AuthScheme = S3SigV4AuthScheme {
    signer: S3SigV4Signer,
};

pub static NO_AUTH: NoAuthScheme = NoAuthScheme {
    signer: NoAuthSigner,
};

/// Every client's schemes, in the order an option is matched to them.
pub static AUTH_SCHEMES: [&dyn AuthScheme; 3] = [&SIGV4, &S3_SIGV4, &NO_AUTH];

/// The anonymous identity, which [`NO_AUTH`] resolves.
static ANONYMOUS: AnonymousIdentityResolver = AnonymousIdentityResolver;

impl AuthScheme for SigV4AuthScheme {
    fn scheme_id(&self) -> AuthSchemeId {
        AuthSchemeId::SIGV4
    }
    fn identity_resolver<'a>(&self, rc: &'a RuntimeComponents) -> &'a dyn ResolveIdentity {
        rc.identity_resolver.as_ref()
    }
    fn signer(&self) -> &dyn Sign {
        &self.signer
    }
}

impl Sign for SigV4Signer {
    fn sign_http_request(
        &self,
        request: &mut HttpRequest,
        identity: &Identity,
        option: &AuthSchemeOption,
        rc: &RuntimeComponents,
    ) -> Result<(), CallError> {
        let credentials = credentials(identity, "SigV4")?;
        let (region, name) = scope(option)?;
        let now = rc.runtime.now();
        super::sign(region, name, credentials, now, request);
        Ok(())
    }
}

/// The option's signing region and name, which [`super::select`] always
/// sets for both SigV4 schemes; without them botocore raises
/// `NoRegionError`.
fn scope(option: &AuthSchemeOption) -> Result<(&str, &str), CallError> {
    match (&option.signing.region, &option.signing.name) {
        (Some(region), Some(name)) => Ok((region, name)),
        _ => Err(CallError::NoRegion),
    }
}

/// The credentials of a credentials identity.
fn credentials<'a>(
    identity: &'a Identity,
    scheme: &str,
) -> Result<&'a SigningCredentials, CallError> {
    match identity {
        Identity::Credentials(credentials) => Ok(credentials),
        Identity::Anonymous => Err(CallError::Internal(format!(
            "{scheme} was given an identity other than credentials"
        ))),
    }
}

impl AuthScheme for S3SigV4AuthScheme {
    fn scheme_id(&self) -> AuthSchemeId {
        AuthSchemeId::S3_SIGV4
    }
    fn identity_resolver<'a>(&self, rc: &'a RuntimeComponents) -> &'a dyn ResolveIdentity {
        rc.identity_resolver.as_ref()
    }
    fn signer(&self) -> &dyn Sign {
        &self.signer
    }
}

impl Sign for S3SigV4Signer {
    /// Signs the payload as `S3SigV4Auth._should_sha256_sign_payload`
    /// decides for a call admission lets through, which has no streaming
    /// input and no unsigned-payload auth type: as the `s3` block's
    /// `payload_signing_enabled` says, when configured; else signed, as
    /// both of botocore's later branches sign such a call (over plain HTTP,
    /// or with `payload_signing_enabled` unset in the request context).
    fn sign_http_request(
        &self,
        request: &mut HttpRequest,
        identity: &Identity,
        option: &AuthSchemeOption,
        rc: &RuntimeComponents,
    ) -> Result<(), CallError> {
        let credentials = credentials(identity, "S3 SigV4")?;
        let (region, service) = scope(option)?;
        let sign_payload = option.signing.payload_signing.unwrap_or(true);
        let scope = sigv4::Scope { region, service };
        super::sign_s3(scope, sign_payload, credentials, rc.runtime.now(), request);
        Ok(())
    }
}

impl AuthScheme for NoAuthScheme {
    fn scheme_id(&self) -> AuthSchemeId {
        AuthSchemeId::NO_AUTH
    }
    fn identity_resolver<'a>(&self, _rc: &'a RuntimeComponents) -> &'a dyn ResolveIdentity {
        &ANONYMOUS
    }
    fn signer(&self) -> &dyn Sign {
        &self.signer
    }
}

impl Sign for NoAuthSigner {
    fn sign_http_request(
        &self,
        _request: &mut HttpRequest,
        _identity: &Identity,
        _option: &AuthSchemeOption,
        _rc: &RuntimeComponents,
    ) -> Result<(), CallError> {
        Ok(())
    }
}
