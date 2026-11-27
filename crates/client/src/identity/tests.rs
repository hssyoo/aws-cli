use super::*;
use crate::credentials::CredentialError;

#[derive(Debug)]
struct Source(Result<Option<SigningCredentials>, CredentialError>);

impl CredentialSource for Source {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        self.0.clone()
    }
    fn method(&self) -> Option<&'static str> {
        Some("env")
    }
}

fn keys() -> SigningCredentials {
    SigningCredentials {
        access_key_id: "AKID".into(),
        secret_access_key: "SECRET".into(),
        session_token: None,
        account_id: Some("123456789012".into()),
    }
}

#[test]
fn a_credential_source_resolves_its_credentials_as_the_identity() {
    let source = Source(Ok(Some(keys())));
    assert!(matches!(
        source.resolve_identity(),
        Ok(Some(Identity::Credentials(found))) if found == keys()
    ));
    assert_eq!(ResolveIdentity::method(&source), Some("env"));
}

#[test]
fn none_found_is_no_identity_and_a_failed_refresh_keeps_its_text() {
    assert!(matches!(Source(Ok(None)).resolve_identity(), Ok(None)));
    let failing = Source(Err(CredentialError(
        "Error when retrieving credentials".into(),
    )));
    assert!(matches!(
        failing.resolve_identity(),
        Err(CallError::Credentials(text)) if text == "Error when retrieving credentials"
    ));
}

#[test]
fn a_source_upcasts_to_the_identity_slot() {
    let source: std::sync::Arc<dyn CredentialSource> = std::sync::Arc::new(Source(Ok(None)));
    let slot: std::sync::Arc<dyn ResolveIdentity> = source;
    assert_eq!(slot.method(), Some("env"));
}

#[test]
fn the_anonymous_resolver_names_no_provider() {
    let resolver = AnonymousIdentityResolver;
    assert!(matches!(
        resolver.resolve_identity(),
        Ok(Some(Identity::Anonymous))
    ));
    assert_eq!(resolver.method(), None);
}

#[test]
fn an_identitys_debug_names_no_secret() {
    let shown = format!("{:?}", Identity::Credentials(keys()));
    assert!(shown.contains("AKID"));
    assert!(!shown.contains("SECRET"));
}
