use super::*;

/// The payload's bundle is trusted only for the embedded setting, and a
/// configured file always wins.
#[test]
fn trust_follows_the_setting_and_the_payload() {
    let mut settings = ClientSettings::new("zquery.get-thing");
    settings.trust.value = Trust::Embedded;
    assert_eq!(
        trust(&settings, Some(b"pem")),
        ::crt::runtime::Trust::Pem(b"pem".to_vec())
    );
    assert_eq!(trust(&settings, None), ::crt::runtime::Trust::Platform);
    settings.trust.value = Trust::Platform;
    assert_eq!(
        trust(&settings, Some(b"pem")),
        ::crt::runtime::Trust::Platform
    );
}
