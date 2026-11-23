#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::ServiceSource;
use crate::testing::{Result, parsed, with_models};

fn input(source: &ServiceSource<'_>, argv: &[&str]) -> Value {
    let result = parsed(source, argv);
    let table = source
        .models()
        .table(argv[0], argv[1])
        .unwrap_or_else(|| panic!("no table"));
    bind(table, &result.values).unwrap_or_else(|reason| panic!("{reason:?}"))
}

fn keys(value: &Value) -> Vec<&str> {
    match value {
        Value::Map(map) => map.keys().collect(),
        other => panic!("{other:?}"),
    }
}

/// The request holds the supplied members in the table's (model) order,
/// whatever argv's, an integer at any magnitude, a boolean's twin as its
/// last spelling, and nothing for what is unset.
#[test]
fn members_bind_in_model_order_as_add_to_params_stores_them() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let value = input(
            &source,
            &[
                "zquery",
                "get-thing",
                "--count",
                "99999999999999999999999",
                "--thing-id",
                "a",
            ],
        );
        assert_eq!(keys(&value), ["ThingId", "Count"]);
        let Value::Map(map) = &value else {
            panic!("{value:?}");
        };
        assert_eq!(
            map.get("Count"),
            Some(&Value::BigInteger("99999999999999999999999".into()))
        );
        let value = input(
            &source,
            &[
                "zquery",
                "get-thing",
                "--thing-id",
                "a",
                "--count",
                " 0_7 ",
                "--enabled",
                "--no-enabled",
            ],
        );
        let Value::Map(map) = &value else {
            panic!("{value:?}");
        };
        assert_eq!(map.get("Count"), Some(&Value::Integer(7)));
        assert_eq!(map.get("Enabled"), Some(&Value::Bool(false)));
        assert_eq!(keys(&value), ["ThingId", "Count", "Enabled"]);
        let value = input(
            &source,
            &["zjson", "measure", "--unit", "u", "--total", "5"],
        );
        assert_eq!(keys(&value), ["Unit", "Total"]);
    })
}
