use super::*;

#[test]
fn an_error_reads_as_its_botocore_text() {
    let error = SerdeError::new("a dict is required");
    assert_eq!(error.to_string(), "a dict is required");
    assert_eq!(error.message(), "a dict is required");
    assert_eq!(error.clone().into_message(), "a dict is required");
    assert_ne!(error, SerdeError::new("a list is required"));
}

/// Every trait is object-safe: codecs and protocols hold them as `dyn`.
#[test]
fn the_traits_are_object_safe() {
    fn takes(
        _: Option<&dyn ShapeSerializer>,
        _: Option<&dyn MapSerializer>,
        _: Option<&dyn SerializableStruct>,
        _: Option<&dyn ShapeDeserializer>,
        _: Option<&dyn Codec>,
        _: Option<Box<dyn FinishSerializer>>,
    ) -> bool {
        true
    }
    assert!(takes(None, None, None, None, None, None));
    assert_ne!(Read::Present, Read::Null);
}
