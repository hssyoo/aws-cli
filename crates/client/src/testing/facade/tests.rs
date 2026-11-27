//! The façade bodies that read the schema rather than the arena give what
//! the arena gave: each test is an input a wrong reading would pass on.
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Two exceptions share a code, a third shape has the code but is no
/// exception, and the operations are out of name order.
const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "json", "jsonVersion": "1.0",
    "targetPrefix": "Z"},
  "operations": {
    "Zeta": {"name": "Zeta", "http": {"method": "POST", "requestUri": "/"},
      "input": {"shape": "In"}},
    "Alpha": {"name": "Alpha", "http": {"method": "POST", "requestUri": "/"},
      "input": {"shape": "In", "locationName": "Root"}}
  },
  "shapes": {
    "In": {"type": "structure", "members": {
      "Ids": {"shape": "Ids", "locationName": "ids"},
      "Plain": {"shape": "Named"}}},
    "Ids": {"type": "list", "member": {"shape": "Id"}},
    "Id": {"type": "integer"},
    "Named": {"type": "string", "locationName": "target-name"},
    "First": {"type": "structure", "members": {}, "exception": true,
      "error": {"code": "Shared"}},
    "Second": {"type": "structure", "members": {}, "exception": true,
      "error": {"code": "Shared"}},
    "Shared": {"type": "structure", "members": {}},
    "Own": {"type": "structure", "members": {}, "exception": true}
  }
}"#;

fn model() -> Result<Arc<Model>, String> {
    model_from_json(MODEL.as_bytes())
}

/// `shape_for_error_code`: the last exception with the code, never a shape
/// that merely has its name; an exception without `error.code` by its
/// name.
#[test]
fn the_error_shape_is_the_last_exception_with_the_code() -> TestResult {
    let model = model()?;
    assert_eq!(
        error_shape_name(&model, "Shared").as_deref(),
        Some("Second")
    );
    assert_eq!(error_shape_name(&model, "Own").as_deref(), Some("Own"));
    assert_eq!(error_shape_name(&model, "First"), None);
    Ok(())
}

/// Model order, not name order; no endpoint prefix when the metadata has
/// none.
#[test]
fn operations_keep_model_order_and_a_missing_prefix_stays_missing() -> TestResult {
    let model = model()?;
    assert_eq!(operation_names(&model), ["Zeta", "Alpha"]);
    assert_eq!(endpoint_prefix(&model), None);
    let prefixed = MODEL.replace(r#""apiVersion""#, r#""endpointPrefix": "zp", "apiVersion""#);
    let prefixed = model_from_json(prefixed.as_bytes())?;
    assert_eq!(endpoint_prefix(&prefixed).as_deref(), Some("zp"));
    Ok(())
}

/// A member's traits are the reference's over its target's, and a list's
/// item type is its member's target's; an operation reference's own traits
/// change none of its members.
#[test]
fn input_members_read_the_effective_traits() -> TestResult {
    let model = model()?;
    for operation in ["Zeta", "Alpha"] {
        let members = input_members(&model, operation);
        let read: Vec<_> = members
            .iter()
            .map(|member| {
                (
                    member.name.as_str(),
                    member.type_name.as_str(),
                    member.item_type_name.as_deref(),
                    member.location_name.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                ("Ids", "list", Some("integer"), Some("ids")),
                ("Plain", "string", None, Some("target-name")),
            ],
            "{operation}"
        );
    }
    Ok(())
}
