//! What a call signs with: credentials read when botocore reads them, at
//! signing (`RequestSigner` calls `get_frozen_credentials`), from the CLI's
//! chain as the `credentials` crate walks it.

/// The credentials one request is signed with (botocore's
/// `ReadOnlyCredentials`, with the account id its credentials carry).
#[derive(Clone, PartialEq, Eq)]
pub struct SigningCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub account_id: Option<String>,
}

/// Names the access key only.
impl std::fmt::Debug for SigningCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningCredentials")
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

/// A refresh that failed, with the text botocore raises for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialError(pub String);

/// Where a client's credentials come from.
pub trait CredentialSource: Send + Sync + std::fmt::Debug {
    /// The credentials to sign one request with, refreshed first if they
    /// are due: `None` for a chain that found none, botocore's
    /// `NoCredentialsError` case.
    ///
    /// # Errors
    /// A refresh botocore raises to the caller.
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError>;
}

/// The chain's result as botocore's session holds it for a client: found
/// once, before the client exists, and refreshed through the provider that
/// found it ([`credentials::Loaded::frozen`]).
#[derive(Debug)]
pub struct ChainCredentials(Option<credentials::Loaded>);

impl ChainCredentials {
    /// What `Resolver::load_credentials` found.
    pub const fn new(loaded: Option<credentials::Loaded>) -> Self {
        Self(loaded)
    }
}

impl CredentialSource for ChainCredentials {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        let Some(loaded) = &self.0 else {
            return Ok(None);
        };
        let frozen = loaded
            .frozen()
            .map_err(|error| CredentialError(error.to_string()))?;
        Ok(Some(SigningCredentials {
            access_key_id: frozen.access_key_id().to_owned(),
            secret_access_key: frozen.secret_access_key().to_owned(),
            session_token: frozen.session_token().map(str::to_owned),
            account_id: frozen.account_id().map(str::to_owned),
        }))
    }
}
