use super::*;
use model::shapes::Model;

/// An operation with a trait the client does not implement is refused,
/// whichever trait it is; a plain one is not.
#[test]
fn operations_with_unimplemented_traits_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let model = Model::from_json(
        br#"{"metadata": {}, "shapes": {}, "operations": {
        "Plain": {"name": "Plain", "http": {"method": "POST", "requestUri": "/"}},
        "Checked": {"name": "Checked", "http": {"method": "POST", "requestUri": "/"},
            "httpChecksum": {"requestChecksumRequired": true}},
        "Required": {"name": "Required", "http": {"method": "POST", "requestUri": "/"},
            "httpChecksumRequired": true},
        "Discovered": {"name": "Discovered", "http": {"method": "POST", "requestUri": "/"},
            "endpointdiscovery": {"required": true}},
        "Compressed": {"name": "Compressed", "http": {"method": "POST", "requestUri": "/"},
            "requestcompression": {"encodings": ["gzip"]}}
    }}"#,
    )?;
    for (name, refused) in [
        ("Plain", None),
        ("Checked", Some("httpChecksum")),
        ("Required", Some("httpChecksumRequired")),
        ("Discovered", Some("endpointdiscovery")),
        ("Compressed", Some("requestcompression")),
    ] {
        let operation = model.operation(name).ok_or(name)?;
        assert_eq!(
            unimplemented_trait(operation),
            refused.map(Unsupported::Trait),
            "{name}"
        );
    }
    Ok(())
}
