//! What a request is signed as, resolved by the auth scheme that signs it
//! ([`crate::auth::AuthScheme::identity_resolver`]): credentials for SigV4,
//! the anonymous identity for `smithy.api#noAuth`.
//!
//! Every [`CredentialSource`] resolves an identity, so the CLI's chain
//! fills the client's identity slot as it is. The orchestrator asks on
//! every attempt and for the deferred account id, and caches nothing: the
//! source refreshes what is due (botocore's `get_frozen_credentials`).
use crate::credentials::{CredentialSource, SigningCredentials};
use crate::error::CallError;

/// An identity a request is signed as. Its `Debug` names no secret, as
/// [`SigningCredentials`]'s names the key only.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Identity {
    /// SigV4 credentials.
    Credentials(SigningCredentials),
    /// No identity: the request goes unsigned.
    Anonymous,
}

/// Where an auth scheme's identity comes from.
pub trait ResolveIdentity: Send + Sync + std::fmt::Debug {
    /// The identity to sign one request as, refreshed first if it is due;
    /// `None` when there is none (`NoCredentialsError`).
    ///
    /// # Errors
    /// [`CallError::Credentials`] for a refresh that failed.
    fn resolve_identity(&self) -> Result<Option<Identity>, CallError>;
    /// botocore's `method` of the provider that answered, for the
    /// `User-Agent`'s credential feature ids; `None` before an identity is
    /// found or for a resolver that cannot say.
    fn method(&self) -> Option<&'static str>;
}

impl<T: CredentialSource> ResolveIdentity for T {
    fn resolve_identity(&self) -> Result<Option<Identity>, CallError> {
        let credentials = self
            .credentials()
            .map_err(|error| CallError::Credentials(error.0))?;
        Ok(credentials.map(Identity::Credentials))
    }
    fn method(&self) -> Option<&'static str> {
        CredentialSource::method(self)
    }
}

/// The `smithy.api#noAuth` scheme's resolver: always the anonymous
/// identity, reading no credentials.
#[derive(Debug)]
pub struct AnonymousIdentityResolver;

impl ResolveIdentity for AnonymousIdentityResolver {
    fn resolve_identity(&self) -> Result<Option<Identity>, CallError> {
        Ok(Some(Identity::Anonymous))
    }
    fn method(&self) -> Option<&'static str> {
        None
    }
}

#[cfg(test)]
mod tests;
