use super::*;
use crate::source::DirSource;
use crate::testing::Result;

fn json(text: &str) -> Result<Value> {
    Ok(Value::from_json(text.as_bytes())?)
}

fn expressions(texts: &[&str]) -> Vec<Expression> {
    texts
        .iter()
        .filter_map(|text| Expression::parse(text))
        .collect()
}

const TWO_TOKENS: &str = r#"{"input_token": ["Marker", "Marker2"],
    "output_token": ["Marker", "Marker2"], "result_key": ["Users", "Groups"]}"#;

#[test]
fn an_entry_reads_as_paginator_reads_it() -> Result<()> {
    let read = Config::from_value(&json(
        r#"{"input_token": "Marker", "output_token": "Inner.Marker", "result_key": "Users",
            "limit_key": "MaxResults", "more_results": "Inner.More",
            "non_aggregate_keys": ["Owner", "Inner.Note"]}"#,
    )?);
    assert_eq!(
        read,
        Ok(Config {
            input_token: vec!["Marker".into()],
            output_token: expressions(&["Inner.Marker"]),
            more_results: Expression::parse("Inner.More"),
            result_key: expressions(&["Users"]),
            non_aggregate_keys: expressions(&["Owner", "Inner.Note"]),
            limit_key: Some("MaxResults".into()),
        })
    );
    let listed = Config::from_value(&json(TWO_TOKENS)?).map_err(|r| format!("{r:?}"))?;
    assert_eq!(listed.input_token, ["Marker", "Marker2"]);
    assert_eq!(listed.result_key, ["Users", "Groups"]);
    assert_eq!((listed.more_results, listed.limit_key), (None, None));
    Ok(())
}

/// The JMESPath the configurations use beyond names and dotted paths:
/// s3's `NextMarker || Contents[-1].Key`, importexport's `Jobs[-1].JobId`.
#[test]
fn an_expression_beyond_a_dotted_path_is_refused() -> Result<()> {
    let refused = |entry: &str, text: &str| -> Result<bool> {
        Ok(Config::from_value(&json(entry)?) == Err(Refused::Expression(text.into())))
    };
    assert!(refused(
        r#"{"input_token": "M", "output_token": ["NextMarker || Contents[-1].Key"],
            "result_key": "U"}"#,
        "NextMarker || Contents[-1].Key"
    )?);
    assert!(refused(
        r#"{"input_token": "M", "output_token": "M", "result_key": "Jobs[-1].JobId"}"#,
        "Jobs[-1].JobId"
    )?);
    assert!(refused(
        r#"{"input_token": "M", "output_token": "M", "result_key": "U",
            "more_results": "\"Quoted\""}"#,
        "\"Quoted\""
    )?);
    for key in ["A.b-c", "A.", "1st"] {
        let entry = format!(
            r#"{{"input_token": "M", "output_token": "M", "result_key": "U",
                "non_aggregate_keys": ["{key}"]}}"#
        );
        assert!(refused(&entry, key)?, "{key}");
    }
    Ok(())
}

/// What botocore cannot read, or cannot page with.
#[test]
fn an_entry_botocore_cannot_read_is_malformed() -> Result<()> {
    let malformed = |entry: &str| -> Result<bool> {
        Ok(matches!(
            Config::from_value(&json(entry)?),
            Err(Refused::Malformed(_))
        ))
    };
    for entry in [
        r#"["Marker"]"#,
        r#"{"input_token": "Marker", "result_key": "Users"}"#,
        r#"{"input_token": "Marker", "output_token": "Marker"}"#,
        r#"{"input_token": "M", "output_token": "M", "result_key": []}"#,
        r#"{"input_token": 5, "output_token": "M", "result_key": "U"}"#,
        r#"{"input_token": "M", "output_token": "M", "result_key": "U", "limit_key": 5}"#,
        r#"{"input_token": "M", "output_token": "M", "result_key": "U",
            "non_aggregate_keys": "Owner"}"#,
    ] {
        assert!(malformed(entry)?, "{entry}");
    }
    Ok(())
}

/// An entry the paginator refuses still gives the table its input tokens
/// and limit key, which `unify_paging_params` reads whatever else it
/// holds; an entry whose tokens are no strings gives none.
#[test]
fn the_table_reads_every_entry_s_tokens() -> Result<()> {
    let refused = Entry::from_value(&json(
        r#"{"input_token": "Marker", "output_token": "Jobs[-1].JobId",
            "result_key": "Jobs", "limit_key": "MaxJobs"}"#,
    )?);
    assert_eq!(refused.input_token, ["Marker"]);
    assert_eq!(refused.limit_key.as_deref(), Some("MaxJobs"));
    assert!(matches!(refused.config, Err(Refused::Expression(_))));
    let mixed = Entry::from_value(&json(r#"{"input_token": ["Marker", 5]}"#)?);
    assert_eq!(mixed.input_token, ["Marker"]);
    assert!(matches!(mixed.config, Err(Refused::Malformed(_))));
    let odd = Entry::from_value(&json(r#"{"input_token": {}, "limit_key": 5}"#)?);
    assert_eq!((odd.input_token.len(), odd.limit_key), (0, None));
    Ok(())
}

/// The fake models' files load by operation; a service without one has
/// none, and so does a service the source does not know.
#[test]
fn a_service_s_file_loads_once_by_operation() -> Result<()> {
    let models = DirSource::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models"));
    let zquery = Paginators::load(&models, "zquery")?;
    let operations: Vec<&str> = zquery.iter().map(|(name, _)| name).collect();
    assert_eq!(
        operations,
        [
            "ListThings",
            "DescribeWidgets",
            "DescribeDefaults",
            "ListJobs"
        ]
    );
    let jobs = zquery.get("ListJobs").ok_or("ListJobs")?;
    assert_eq!(
        jobs.config,
        Err(Refused::Expression("Jobs[-1].JobId".into()))
    );
    assert!(zquery.get("GetThing").is_none());
    assert_eq!(Paginators::load(&models, "zglobal")?, Paginators::default());
    assert_eq!(Paginators::load(&models, "nosuch")?, Paginators::default());
    Ok(())
}
