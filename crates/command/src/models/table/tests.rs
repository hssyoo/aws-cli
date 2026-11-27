#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;

/// One service whose operations cover what the table builder decides:
/// typed members, a streaming output, and paging with each limit type.
const MODEL: &str = r#"{
  "metadata": {"protocol": "rest-json", "apiVersion": "2020-01-01",
               "endpointPrefix": "zsvc", "serviceId": "Zsvc"},
  "operations": {
    "Measure": {"name": "Measure", "http": {"method": "POST", "requestUri": "/"},
                "input": {"shape": "MeasureRequest"}},
    "Download": {"name": "Download", "http": {"method": "GET", "requestUri": "/"},
                 "input": {"shape": "ListRequest"}, "output": {"shape": "DownloadResult"}},
    "ListThings": {"name": "ListThings", "http": {"method": "GET", "requestUri": "/"},
                   "input": {"shape": "ListRequest"}},
    "ListShadowed": {"name": "ListShadowed", "http": {"method": "GET", "requestUri": "/"},
                     "input": {"shape": "ShadowedRequest"}}
  },
  "shapes": {
    "Long": {"type": "long"}, "Double": {"type": "double"}, "Float": {"type": "float"},
    "Integer": {"type": "integer"}, "String": {"type": "string"},
    "Body": {"type": "blob", "streaming": true},
    "MeasureRequest": {"type": "structure", "members": {
      "Total": {"shape": "Long"}, "Ratio": {"shape": "Double"},
      "Share": {"shape": "Float"}, "Count": {"shape": "Integer"}}},
    "ListRequest": {"type": "structure", "members": {
      "NextToken": {"shape": "String"}, "MaxResults": {"shape": "Integer"},
      "Marker": {"shape": "String"}}},
    "ShadowedRequest": {"type": "structure", "members": {
      "StartingToken": {"shape": "String"}, "MaxItems": {"shape": "Integer"},
      "Filter": {"shape": "String"}}},
    "DownloadResult": {"type": "structure", "payload": "Body",
                       "members": {"Body": {"shape": "Body"}}}
  }
}"#;

fn table(operation: &str, pages: Option<&Json>) -> Table {
    let model = Model::from_json(MODEL.as_bytes()).unwrap_or_else(|error| panic!("{error}"));
    let special = SpecialCases::default();
    let operation = model
        .schema()
        .service()
        .operation(operation)
        .unwrap_or_else(|| panic!("no {operation}"));
    let entry = pages.map(|json| Entry::from_value(&model::value::Value::from_serde(json)));
    derive(operation, entry.as_ref(), &special)
}

fn find<'t>(table: &'t Table, name: &str) -> &'t DerivedArgument {
    table
        .arguments
        .iter()
        .find(|argument| &*argument.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn long_and_floating_members_parse_through_argparse_types() {
    let table = table("Measure", None);
    for (name, value_type) in [
        ("total", Some(ValueType::Int)),
        ("ratio", Some(ValueType::Float)),
        ("share", Some(ValueType::Float)),
        ("count", None),
    ] {
        assert_eq!(find(&table, name).value_type, value_type, "{name}");
    }
    assert!(table.typed());
}

#[test]
fn a_streaming_output_takes_outfile_and_drops_the_input_options() {
    let table = table("Download", None);
    let outfile = find(&table, "outfile");
    assert!(outfile.positional);
    assert_eq!(
        outfile.origin,
        Origin::Outfile {
            response_key: "Body".into()
        }
    );
    for name in ["cli-input-json", "cli-input-yaml", "generate-cli-skeleton"] {
        assert!(
            !table
                .arguments
                .iter()
                .any(|argument| &*argument.name == name),
            "{name}"
        );
    }
    assert!(table.overrides.is_empty());
}

#[test]
fn paging_hides_the_tokens_and_types_the_page_options_by_the_limit_key() {
    let integer: Json = serde_json::json!({"input_token": "NextToken", "limit_key": "MaxResults"});
    let table = table("ListThings", Some(&integer));
    assert!(find(&table, "next-token").hidden && find(&table, "max-results").hidden);
    assert_eq!(find(&table, "page-size").cli_type_name.as_ref(), "integer");
    assert_eq!(find(&table, "max-items").value_type, Some(ValueType::Int));
    let names: Vec<&str> = table
        .arguments
        .iter()
        .map(|argument| &*argument.name)
        .collect();
    assert_eq!(
        names[names.len() - 4..],
        [
            "starting-token",
            "page-size",
            "max-items",
            "generate-cli-skeleton"
        ]
    );
    assert!(!table.customized);
    let string: Json = serde_json::json!({"input_token": "NextToken", "limit_key": "Marker"});
    let table = table_for_string_limit(&string);
    assert_eq!(find(&table, "max-items").value_type, None);
    let no_limit: Json = serde_json::json!({"input_token": "NextToken"});
    let table = table_for_string_limit(&no_limit);
    assert!(
        !table
            .arguments
            .iter()
            .any(|argument| &*argument.name == "page-size")
    );
    assert_eq!(find(&table, "max-items").cli_type_name.as_ref(), "integer");
}

fn table_for_string_limit(config: &Json) -> Table {
    table("ListThings", Some(config))
}

#[test]
fn a_paging_token_with_no_argument_is_python_s_to_build() {
    let missing: Json = serde_json::json!({"input_token": "Cursor"});
    let table = table("ListThings", Some(&missing));
    assert!(table.customized);
    assert_eq!(table.paging, None);
}

/// `unify_paging_params` hands its handlers the tokens and the limit key,
/// in the model's names and the CLI's, and nothing shadowed when no member
/// is named as a page option; the page options bind as they parse.
#[test]
fn paging_records_its_tokens_for_the_parse() {
    let config: Json = serde_json::json!({"input_token": "NextToken", "limit_key": "MaxResults"});
    let table = table("ListThings", Some(&config));
    let paging = table.paging.as_ref().unwrap_or_else(|| panic!("no paging"));
    let pairs: Vec<(&str, &str)> = paging
        .tokens
        .iter()
        .map(|token| (&*token.model, &*token.cli))
        .collect();
    assert_eq!(
        pairs,
        [("NextToken", "next-token"), ("MaxResults", "max-results")]
    );
    assert!(paging.shadowed.is_empty());
    for name in ["starting-token", "page-size", "max-items"] {
        assert_eq!(find(&table, name).unpack, Unpack::AsIs, "{name}");
    }
}

/// A member named as a page option is replaced where it stands and kept
/// as shadowed (`add_paging_argument`), hidden as a token is.
#[test]
fn a_member_named_as_a_page_option_is_shadowed_in_place() {
    let config: Json = serde_json::json!({"input_token": "StartingToken", "limit_key": "MaxItems"});
    let table = table("ListShadowed", Some(&config));
    let names: Vec<&str> = table
        .arguments
        .iter()
        .map(|argument| &*argument.name)
        .collect();
    assert_eq!(
        names[..4],
        ["starting-token", "max-items", "filter", "cli-input-json"]
    );
    assert_eq!(find(&table, "starting-token").origin, Origin::StartingToken);
    assert_eq!(find(&table, "max-items").origin, Origin::MaxItems);
    let paging = table.paging.as_ref().unwrap_or_else(|| panic!("no paging"));
    let shadowed: Vec<(&str, bool)> = paging
        .shadowed
        .iter()
        .map(|argument| (&*argument.name, argument.hidden))
        .collect();
    assert_eq!(shadowed, [("starting-token", true), ("max-items", true)]);
    assert!(matches!(
        &paging.shadowed[1].origin,
        Origin::Member { member, .. } if &**member == "MaxItems"
    ));
}

/// What the unpack loop does with each member's supplied value: a string,
/// long or boolean as it stands, an integer through `int()`, and the rest,
/// with every customization's option, only on the Python route.
#[test]
fn each_argument_says_how_its_value_unpacks() {
    let table = table("Measure", None);
    for (name, unpack) in [
        ("total", Unpack::AsIs),
        ("count", Unpack::Integer),
        ("ratio", Unpack::Deferred),
        ("share", Unpack::Deferred),
        ("cli-input-json", Unpack::Deferred),
        ("generate-cli-skeleton", Unpack::Deferred),
    ] {
        assert_eq!(find(&table, name).unpack, unpack, "{name}");
    }
    let table = self::table("ListThings", None);
    assert_eq!(find(&table, "next-token").unpack, Unpack::AsIs);
}
