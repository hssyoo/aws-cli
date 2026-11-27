//! The binder, and Smithy's subset of the paths it binds. The engine over
//! [`model::value::Value`]s and its oracles are `model::pyjmespath`'s.
use super::Python;
use crate::endpoint::ParamValue;
use model::pyjson;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const RULES: &str = r#"{"version": "1.0", "parameters": {
    "Arns": {"type": "stringArray", "required": false},
    "Arn": {"type": "String", "required": false}},
    "rules": [{"conditions": [], "type": "endpoint",
        "endpoint": {"url": "https://example.com"}}]}"#;

const SERVICE: &str = r#"{"metadata": {"protocol": "json", "jsonVersion": "1.0"},
    "clientContextParams": {"Arn": {"type": "string"}},
    "operations": {
        "Batch": {"name": "Batch", "http": {"method": "POST", "requestUri": "/"},
            "operationContextParams": {"Arns": {"path": "keys(RequestItems)"}}},
        "Transact": {"name": "Transact", "http": {"method": "POST", "requestUri": "/"},
            "operationContextParams": {"Arns":
                {"path": "TransactItems[*].[Get.TableName, Put.TableName][]"}}},
        "Filtered": {"name": "Filtered", "http": {"method": "POST", "requestUri": "/"},
            "operationContextParams": {"Arns": {"path": "Items[?Name].Name"}}},
        "Broken": {"name": "Broken", "http": {"method": "POST", "requestUri": "/"},
            "operationContextParams": {"Arns": {"path": "keys(Name)"}}},
        "Pinned": {"name": "Pinned", "http": {"method": "POST", "requestUri": "/"},
            "input": {"shape": "In"},
            "staticContextParams": {"Arn": {"value": "static"}},
            "operationContextParams": {"Arn": {"path": "Name"}}},
        "Member": {"name": "Member", "http": {"method": "POST", "requestUri": "/"},
            "input": {"shape": "In"},
            "operationContextParams": {"Arn": {"path": "Other"}}}
    },
    "shapes": {
        "In": {"type": "structure", "members": {
            "Name": {"shape": "S", "contextParam": {"name": "Arn"}}, "Other": {"shape": "S"}}},
        "S": {"type": "string"}}}"#;

/// The parameters `operation` resolves to for the input `json`.
fn bound(operation: &str, json: &str) -> Result<Vec<(String, ParamValue)>> {
    let model = model::shapes::Model::from_json(SERVICE.as_bytes())?;
    let data = super::super::EndpointData::from_documents(
        RULES.as_bytes().to_vec(),
        b"{}".to_vec(),
        None,
    )?;
    let operation = model
        .schema()
        .service()
        .operation(operation)
        .ok_or("no operation")?;
    let input = pyjson::loads(json).map_err(|e| format!("{e:?}"))?;
    let builtins = super::super::Builtins::default();
    let params = super::super::provider_params(&data, operation, &input, &builtins, None);
    Ok(params
        .iter()
        .map(|(name, value)| (name.to_owned(), value.clone()))
        .collect())
}

fn strings(items: &[&str]) -> ParamValue {
    ParamValue::StringArray(items.iter().map(|item| (*item).to_owned()).collect())
}

/// A path's result binds its parameter as a context parameter's would:
/// keys in the dict's order, a projection's strings, none when nothing is
/// found; after the static value and the input member, and before the
/// client context.
#[test]
fn operation_context_params_bind_after_the_operation_s_own() -> Result {
    let arns = |found: ParamValue| vec![("Arns".to_owned(), found)];
    let batch = r#"{"RequestItems": {"t2": {}, "t1": {}}}"#;
    assert_eq!(bound("Batch", batch)?, arns(strings(&["t2", "t1"])));
    let transact =
        r#"{"TransactItems": [{"Get": {"TableName": "a"}}, {"Put": {"TableName": "b"}}]}"#;
    assert_eq!(bound("Transact", transact)?, arns(strings(&["a", "b"])));
    assert_eq!(bound("Batch", "{}")?, []);
    assert_eq!(
        bound("Batch", r#"{"RequestItems": {}}"#)?,
        arns(strings(&[]))
    );
    let arn = |found: &str| vec![("Arn".to_owned(), ParamValue::String(found.to_owned()))];
    assert_eq!(bound("Pinned", r#"{"Name": "member"}"#)?, arn("static"));
    assert_eq!(
        bound("Member", r#"{"Name": "member", "Other": "path"}"#)?,
        arn("member")
    );
    assert_eq!(bound("Member", r#"{"Other": "path"}"#)?, arn("path"));
    // A list holding anything but strings coerces to nothing.
    assert_eq!(
        bound(
            "Transact",
            r#"{"TransactItems": [{"Get": {"TableName": 1}}]}"#
        )?,
        []
    );
    Ok(())
}

/// A path outside Smithy's subset, or one whose evaluation raises, binds
/// nothing; and a path names a client-context parameter as bound.
#[test]
fn unsupported_or_failing_paths_bind_nothing() -> Result {
    assert_eq!(bound("Filtered", r#"{"Items": [{"Name": "a"}]}"#)?, []);
    assert_eq!(bound("Broken", r#"{"Name": "a"}"#)?, []);
    let model = model::shapes::Model::from_json(SERVICE.as_bytes())?;
    let rules = r#"{"version": "1.0", "parameters": {"Arn": {"type": "String", "required": true}},
        "rules": [{"conditions": [], "error": "unused", "type": "error"}]}"#;
    let data = super::super::EndpointData::from_documents(
        rules.as_bytes().to_vec(),
        b"{}".to_vec(),
        None,
    )?;
    let service = model.schema().service();
    let member = service.operation("Member").ok_or("Member")?;
    let batch = service.operation("Batch").ok_or("Batch")?;
    assert_eq!(
        super::super::missing_client_context(&data, &model, member),
        None
    );
    assert_eq!(
        super::super::missing_client_context(&data, &model, batch),
        Some("Arn".to_owned())
    );
    Ok(())
}

/// Smithy's `operationContextParams` subset: every path the models ship
/// is within it; filters, slices, indexes, object wildcards, pipes,
/// literals and any function but `keys` are not.
#[test]
fn the_operation_context_params_subset() -> Result {
    use jmespath::{Expression, Feature, OPERATION_CONTEXT_PARAMS, Outside};
    let check = |text: &str| -> Result<std::result::Result<(), Outside>> {
        Ok(Expression::parse(&Python, text)?.check(&OPERATION_CONTEXT_PARAMS))
    };
    for path in [
        "TableCreationParameters.TableName",
        "TransactItems[*].Get.TableName",
        concat!(
            "TransactItems[*].[ConditionCheck.TableName, Put.TableName, ",
            "Delete.TableName, Update.TableName][]"
        ),
        "keys(RequestItems)",
        "keys(@)",
        "\"quoted\".name",
    ] {
        assert_eq!(check(path)?, Ok(()), "{path}");
    }
    let feature = |feature| Err(Outside::Feature(feature));
    let function = || Err(Outside::Function("values".into()));
    for (path, refused) in [
        ("Items[?Name].Name", feature(Feature::Filter)),
        ("Items[0]", feature(Feature::Index)),
        ("Items[1:]", feature(Feature::Slice)),
        ("Items.*", feature(Feature::ObjectWildcard)),
        ("a | b", feature(Feature::Pipe)),
        ("{a: b}", feature(Feature::MultiSelectHash)),
        ("a || b", feature(Feature::Logic)),
        ("keys(`{}`)", feature(Feature::Literal)),
        ("a == b", feature(Feature::Comparator)),
        ("values(a)", function()),
        ("keys(values(a))", function()),
    ] {
        assert_eq!(check(path)?, refused, "{path}");
    }
    Ok(())
}
