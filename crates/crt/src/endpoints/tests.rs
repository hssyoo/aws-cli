use super::{Documents, ParamValue, Resolved, resolve};

/// A rule set with one parameter of each kind the engine takes: an
/// endpoint that templates the second name of a list under FIPS, a plain
/// FIPS endpoint, and an error otherwise.
const RULES: &str = r#"{
    "version": "1.0",
    "parameters": {
        "Region": {"type": "String", "required": true},
        "UseFIPS": {"type": "Boolean", "required": true, "default": false},
        "Names": {"type": "stringArray", "required": false}
    },
    "rules": [
        {
            "conditions": [
                {"fn": "booleanEquals", "argv": [{"ref": "UseFIPS"}, true]},
                {"fn": "isSet", "argv": [{"ref": "Names"}]},
                {"fn": "getAttr", "argv": [{"ref": "Names"}, "[1]"], "assign": "second"}
            ],
            "endpoint": {"url": "https://{second}.fips.{Region}.example.com"},
            "type": "endpoint"
        },
        {
            "conditions": [{"fn": "booleanEquals", "argv": [{"ref": "UseFIPS"}, true]}],
            "endpoint": {"url": "https://fips.{Region}.example.com"},
            "type": "endpoint"
        },
        {"conditions": [], "error": "no FIPS in {Region}", "type": "error"}
    ]
}"#;

const PARTITIONS: &str = r#"{
    "version": "1.1",
    "partitions": [{
        "id": "aws",
        "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$",
        "regions": {},
        "outputs": {
            "name": "aws",
            "dnsSuffix": "example.com",
            "dualStackDnsSuffix": "example.com",
            "supportsFIPS": true,
            "supportsDualStack": true,
            "implicitGlobalRegion": "us-east-1"
        }
    }]
}"#;

fn resolved(parameters: &[(&str, ParamValue<'_>)]) -> Result<Resolved, super::ResolveError> {
    let documents = Documents {
        rules_json: RULES.as_bytes(),
        partitions_json: PARTITIONS.as_bytes(),
    };
    resolve(documents, parameters)
}

fn endpoint(url: &str) -> Resolved {
    Resolved::Endpoint {
        url: url.as_bytes().to_vec(),
        properties: Vec::new(),
    }
}

/// Each parameter reaches the engine by its name, whatever its kind: a
/// string, a boolean and a list of strings.
#[test]
#[cfg_attr(miri, ignore = "calls the CRT, which Miri cannot run")]
fn parameters_reach_the_engine_by_name() -> Result<(), Box<dyn std::error::Error>> {
    let names = ["first", "second"];
    assert_eq!(
        resolved(&[
            ("Region", ParamValue::String("us-west-2")),
            ("UseFIPS", ParamValue::Boolean(true)),
            ("Names", ParamValue::StringArray(&names)),
        ])?,
        endpoint("https://second.fips.us-west-2.example.com")
    );
    assert_eq!(
        resolved(&[
            ("UseFIPS", ParamValue::Boolean(true)),
            ("Region", ParamValue::String("eu-west-1")),
        ])?,
        endpoint("https://fips.eu-west-1.example.com")
    );
    assert_eq!(
        resolved(&[("Region", ParamValue::String("us-west-2"))])?,
        Resolved::RuleError(b"no FIPS in us-west-2".to_vec())
    );
    Ok(())
}

/// A name given twice takes its last value, as the engine's context keeps
/// the last one added; an empty list is a list that is set.
#[test]
#[cfg_attr(miri, ignore = "calls the CRT, which Miri cannot run")]
fn a_later_value_of_a_name_replaces_an_earlier_one() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        resolved(&[
            ("Region", ParamValue::String("us-west-2")),
            ("UseFIPS", ParamValue::Boolean(true)),
            ("UseFIPS", ParamValue::Boolean(false)),
        ])?,
        Resolved::RuleError(b"no FIPS in us-west-2".to_vec())
    );
    assert_eq!(
        resolved(&[
            ("Region", ParamValue::String("us-west-2")),
            ("UseFIPS", ParamValue::Boolean(true)),
            ("Names", ParamValue::StringArray(&[])),
        ])?,
        endpoint("https://fips.us-west-2.example.com")
    );
    Ok(())
}
