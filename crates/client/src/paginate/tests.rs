use super::*;

/// The scaffold reads no entry yet.
#[test]
fn an_entry_is_not_read_yet() {
    assert!(Config::from_value(&Value::Map(Map::new())).is_err());
}
