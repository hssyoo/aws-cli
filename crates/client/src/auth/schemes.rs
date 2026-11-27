//! The auth schemes a client serves: SigV4 (`SigV4Auth`) and none.
use super::{AuthScheme, AuthSchemeId, AuthSchemeOption, Sign};
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

pub static NO_AUTH: NoAuthScheme = NoAuthScheme {
    signer: NoAuthSigner,
};

/// Every client's schemes, in the order an option is matched to them.
pub static AUTH_SCHEMES: [&dyn AuthScheme; 2] = [&SIGV4, &NO_AUTH];

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
    /// The option's region and name, which [`super::select`] always sets
    /// for SigV4; without them botocore raises `NoRegionError`.
    fn sign_http_request(
        &self,
        request: &mut HttpRequest,
        identity: &Identity,
        option: &AuthSchemeOption,
        rc: &RuntimeComponents,
    ) -> Result<(), CallError> {
        let Identity::Credentials(credentials) = identity else {
            return Err(CallError::Internal(
                "SigV4 was given an identity other than credentials".to_owned(),
            ));
        };
        let (Some(region), Some(name)) = (&option.signing.region, &option.signing.name) else {
            return Err(CallError::NoRegion);
        };
        let now = rc.runtime.now();
        super::sign(region, name, credentials, now, request);
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
