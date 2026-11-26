//! Every operation of every shipped model, through the refusals the model
//! and the rule set decide alone, with the CLI's customizations: the
//! operations each refusal sends to Python, printed as a table. It asserts
//! the table's structure, not its numbers: every refusal names what it
//! refuses, and an operation that reaches a key the model loader does not
//! interpret is refused with that key's name.
use client::testing::facade;
use model::source::{DirSource, Kind, Source};
use std::collections::BTreeMap;
use std::path::PathBuf;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The static refusal's (kind, name) of every operation of `service` in
/// `source`, with this crate's table.
fn refusals(source: &dyn Source, service: &str) -> Result<Vec<Option<(String, String)>>> {
    let table = &customizations::TABLE[..];
    let rows = facade::refusals(source, service, table)?;
    Ok(rows.into_iter().map(|(_, refusal)| refusal).collect())
}

/// An operation, by name.
struct Operation {
    name: String,
}

#[test]
fn every_shipped_operation_is_admitted_or_refused_by_name() -> Result<()> {
    let source = DirSource::new(repository().join("aws-cli/awscli/botocore/data"));
    let services = source.services(Kind::Service)?;
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let (mut operations, mut admitted) = (0, 0);
    for service in &services {
        for refusal in refusals(&source, service).map_err(|e| format!("{service}: {e}"))? {
            operations += 1;
            match refusal {
                None => admitted += 1,
                Some(refusal) => {
                    let (kind, name) = refusal;
                    assert!(!name.is_empty(), "{service}: an unnamed {kind} refusal");
                    *counts.entry((kind, name)).or_default() += 1;
                }
            }
        }
    }
    let mut table = format!(
        "{} services, {operations} operations, {admitted} pass the static refusals\n",
        services.len()
    );
    for ((kind, name), count) in &counts {
        table.push_str(&format!("{count:>7}  {kind} {name}\n"));
    }
    std::io::Write::write_all(&mut std::io::stderr(), table.as_bytes())?;
    assert_eq!(admitted + counts.values().sum::<usize>(), operations);
    assert!(admitted > 0 && !counts.is_empty());
    Ok(())
}

/// The fake json model with a key the loader does not interpret planted on
/// one operation: that operation is refused with the key's name, the other
/// operations are not refused for it.
#[test]
fn an_uninterpreted_key_refuses_the_operation_by_its_name() -> Result<()> {
    let models = repository().join("tests/models");
    let scratch = std::env::temp_dir().join(format!("client-census-{}", std::process::id()));
    let directory = scratch.join("zjson/2020-01-01");
    std::fs::create_dir_all(&directory)?;
    let original = models.join("zjson/2020-01-01");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(original.join("service-2.json"))?)?;
    json["operations"]["Ping"]["aws.api#longPoll"] = serde_json::json!({});
    std::fs::write(directory.join("service-2.json"), serde_json::to_vec(&json)?)?;
    let rules = std::fs::read(original.join("endpoint-rule-set-1.json"))?;
    std::fs::write(directory.join("endpoint-rule-set-1.json"), rules)?;
    for data in ["partitions", "endpoints"] {
        let file = format!("{data}.json");
        let botocore = repository()
            .join("aws-cli/awscli/botocore/data")
            .join(&file);
        std::fs::copy(botocore, scratch.join(&file))?;
    }
    let source = DirSource::new(&scratch);
    let model = facade::load_model(&source, "zjson")?;
    let operations: Vec<Operation> = facade::operation_names(&model)
        .into_iter()
        .map(|name| Operation { name })
        .collect();
    let found = refusals(&source, "zjson");
    std::fs::remove_dir_all(&scratch)?;
    let found = found?;
    for (operation, refusal) in operations.iter().zip(found) {
        let planted = Some(("trait".to_owned(), "aws.api#longPoll".to_owned()));
        if &*operation.name == "Ping" {
            assert_eq!(refusal, planted);
        } else {
            assert_ne!(refusal, planted, "{}", operation.name);
        }
    }
    Ok(())
}
