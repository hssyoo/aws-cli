use super::census::{
    Census, Error, Ignored, census, paginator_features, rule_set_features, service_features,
    string_list, waiter_features,
};
use super::matrix::{listed, problems, summary};
use serde_json::{Value as Json, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Error>;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ignored() -> Result<Ignored> {
    Ignored::read(&repository())
}

fn features(build: impl FnOnce(&mut BTreeSet<String>)) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    build(&mut out);
    out
}

fn census_of(features: &[&str]) -> Census {
    let service = BTreeSet::from(["svc".to_owned()]);
    features
        .iter()
        .map(|feature| ((*feature).to_owned(), service.clone()))
        .collect()
}

/// A json 1.1 model with one operation, `Put`, whose input carries the
/// given member, and an unreachable shape with a wire trait.
fn model(member: Json) -> Json {
    json!({
        "version": "2.0",
        "documentation": "ignored",
        "metadata": {"protocol": "json", "jsonVersion": "1.1", "serviceFullName": "ignored",
                     "auth": ["aws.auth#sigv4"]},
        "operations": {"Put": {"name": "Put", "http": {"method": "POST", "requestUri": "/"},
                               "input": {"shape": "PutInput"}, "readonly": true,
                               "documentation": "ignored"}},
        "shapes": {
            "PutInput": {"type": "structure", "members": {"Item": member}, "required": ["Item"]},
            "Text": {"type": "string", "max": 10},
            "When": {"type": "timestamp"},
            "Orphan": {"type": "blob", "streaming": true}
        }
    })
}

/// The completeness check over the 436 shipped models and the fakes.
#[test]
fn every_shipped_feature_has_a_fake_or_an_open_gap() -> Result<()> {
    let ignored = ignored()?;
    let real = census(&repository().join("aws-cli/awscli/botocore/data"), &ignored)?;
    let fakes = census(&repository().join("tests/models"), &ignored)?;
    let text =
        std::fs::read_to_string(repository().join("crates/model/tests/fake_matrix/gaps.txt"))?;
    let gaps = listed(&text);
    let table = summary(&real, &fakes, &gaps);
    std::io::Write::write_all(&mut std::io::stderr(), table.as_bytes())?;
    assert!(
        real.len() > 400 && fakes.len() > 100,
        "the census found too little"
    );
    let found = problems(&real, &fakes, &gaps);
    assert!(
        found.is_empty(),
        "{} problems:\n{}",
        found.len(),
        found.join("\n")
    );
    Ok(())
}

#[test]
fn the_ignored_lists_are_read_from_the_model_loader() -> Result<()> {
    let source = std::fs::read_to_string(repository().join("crates/model/src/shapes/keys.rs"))?;
    let operation = string_list(&source, "OPERATION_IGNORED")?;
    assert!(operation.contains("readonly") && operation.contains("documentation"));
    assert_eq!(
        string_list(&source, "HTTP_IGNORED")?,
        BTreeSet::from(["responseCode".into()])
    );
    ignored()?;
    Ok(())
}

/// Evasion: a renamed or emptied list in `keys.rs` would make every key a
/// feature, or none; the census refuses to run instead.
#[test]
fn a_missing_or_empty_list_is_an_error() {
    assert!(string_list("const A: &[&str] = &[\"x\"];", "B").is_err());
    assert!(string_list("const A: &[&str] = &[];", "A").is_err());
    assert!(string_list("const A: &[&str] = &[\"x\"", "A").is_err());
}

#[test]
fn a_service_model_yields_its_features_and_skips_ignored_keys() -> Result<()> {
    let ignored = ignored()?;
    let member = json!({"shape": "Text", "location": "header", "locationName": "x-item",
                        "documentation": "ignored"});
    let found = features(|out| service_features(&model(member), &ignored, out));
    let expected = [
        "meta/protocol",
        "meta/protocol/json",
        "meta/jsonVersion",
        "meta/jsonVersion/1.1",
        "meta/auth",
        "meta/auth/aws.auth#sigv4",
        "op/json/http",
        "op/json/http/method/POST",
        "op/json/input",
        "shape/json/in/structure",
        "trait/structure/required",
        "member/json/in/string/header",
        "member/json/in/string/header/locationName",
        "shape/json/in/string",
        "trait/string/max",
    ];
    let expected: BTreeSet<String> = expected.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(found, expected);
    Ok(())
}

/// Evasion: a fake declaring a shape no operation reaches, to cover a wire
/// trait it never sends or parses. Unreachable shapes yield nothing
/// (`Orphan` in [`model`]).
#[test]
fn an_unreachable_shape_is_no_feature() -> Result<()> {
    let ignored = ignored()?;
    let found = features(|out| service_features(&model(json!({"shape": "Text"})), &ignored, out));
    assert!(
        !found
            .iter()
            .any(|feature| feature.contains("streaming") || feature.contains("blob"))
    );
    Ok(())
}

/// Evasion: covering a parsed (output) feature with a serialized (input)
/// one, or a rest-xml feature with a json one. Direction and protocol are
/// part of the name.
#[test]
fn direction_and_protocol_are_part_of_a_wire_feature() -> Result<()> {
    let ignored = ignored()?;
    let member = json!({"shape": "When", "timestampFormat": "iso8601"});
    let mut input = model(member);
    let found = features(|out| service_features(&input, &ignored, out));
    assert!(found.contains("member/json/in/timestamp/body/timestampFormat"));
    input["operations"]["Put"]["output"] = input["operations"]["Put"]["input"].take();
    input["metadata"]["protocol"] = json!("rest-xml");
    let found = features(|out| service_features(&input, &ignored, out));
    assert!(found.contains("member/rest-xml/out/timestamp/body/timestampFormat"));
    assert!(
        !found
            .iter()
            .any(|feature| feature.contains("/in/") || feature.contains("/json/"))
    );
    Ok(())
}

#[test]
fn operation_forms_are_features() -> Result<()> {
    let ignored = ignored()?;
    let mut json = model(json!({"shape": "Text"}));
    let operation = &mut json["operations"]["Put"];
    operation["http"] = json!({"method": "PUT", "requestUri": "/{Key+}?x", "responseCode": 200});
    operation["endpoint"] = json!({"hostPrefix": "{Account}."});
    operation["authtype"] = json!("none");
    operation["requestcompression"] = json!({"encodings": ["gzip"]});
    operation["staticContextParams"] = json!({"UseS3": {"value": true}});
    operation["errors"] = json!([{"shape": "Text", "exception": true}]);
    let found = features(|out| service_features(&json, &ignored, out));
    for feature in [
        "op/json/uri/label",
        "op/json/uri/greedy",
        "op/json/uri/query",
        "op/json/hostPrefix/label",
        "op/authtype/none",
        "op/json/requestcompression",
        "op/requestcompression/encodings/gzip",
        "op/staticContextParams/bool",
        "op/json/errors/exception",
        "shape/json/out/string",
    ] {
        assert!(found.contains(feature), "{feature} not in {found:?}");
    }
    assert!(!found.iter().any(|feature| feature.contains("responseCode")));
    Ok(())
}

#[test]
fn rule_set_paginator_and_waiter_forms_are_features() {
    let rules = json!({
        "parameters": {"Region": {"type": "String", "builtIn": "AWS::Region", "required": true}},
        "rules": [{"type": "tree", "conditions": [{"fn": "isSet", "argv": [{"ref": "Region"}]}],
                   "rules": [{"type": "endpoint", "conditions": [],
                              "endpoint": {"url": "https://x", "properties": {"authSchemes": [
                                  {"name": "sigv4a", "signingRegionSet": ["*"]}]}}}]}]
    });
    let found = features(|out| rule_set_features(&rules, out));
    for feature in [
        "rules/param/string",
        "rules/builtIn/AWS::Region",
        "rules/param-key/required",
        "rules/fn/isSet",
        "rules/rule/tree",
        "rules/rule/endpoint",
        "rules/endpoint/properties",
        "rules/property/authSchemes",
        "rules/authScheme/sigv4a",
        "rules/authScheme-key/signingRegionSet",
    ] {
        assert!(found.contains(feature), "{feature} not in {found:?}");
    }
    let pages = json!({"pagination": {"List": {"input_token": ["A", "B"],
        "output_token": "Next || Marker", "more_results": "Page.Truncated",
        "result_key": "Items[-1]"}}});
    let found = features(|out| paginator_features(&pages, out));
    for feature in [
        "pages/input_token/list",
        "pages/output_token/or",
        "pages/more_results/dotted",
        "pages/result_key/index",
    ] {
        assert!(found.contains(feature), "{feature} not in {found:?}");
    }
    let acceptor = json!({"matcher": "status", "state": "retry", "expected": 404});
    let waiters = json!({"version": 2, "waiters": {"Done": {"operation": "Get", "delay": 5,
        "maxAttempts": 3, "acceptors": [acceptor]}}});
    let found = features(|out| waiter_features(&waiters, out));
    assert!(found.contains("wait/status/retry") && found.contains("wait/status/expected/int"));
    assert!(found.contains("wait/key/maxAttempts"));
}

#[test]
fn a_new_shipped_feature_fails_naming_it() {
    let real = census_of(&["a/x", "b/y"]);
    let found = problems(&real, &census_of(&["a/x"]), &[]);
    assert_eq!(found.len(), 1);
    assert!(found[0].starts_with("feature b/y is used by 1 shipped models (e.g. svc)"));
    assert!(problems(&real, &census_of(&["a/x"]), &["b/y"]).is_empty());
}

/// Evasion: a gap left listed after a fake covers it would let the fake
/// regress unseen; it fails as stale.
#[test]
fn a_covered_gap_is_stale() {
    let found = problems(&census_of(&["a/x"]), &census_of(&["a/x"]), &["a/x"]);
    assert_eq!(
        found,
        ["gaps.txt line a/x is stale: a fake now exercises it; delete the line"]
    );
}

/// Evasion: listing features no model uses yet, to pre-allow future gaps.
#[test]
fn a_gap_no_shipped_model_uses_is_stale() {
    let found = problems(&census_of(&[]), &census_of(&[]), &["future/trait"]);
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("no shipped model uses it"));
}

/// Evasion: a repeated or unsorted line hides a second copy of an entry
/// from review.
#[test]
fn gaps_must_be_sorted_and_unique() {
    let real = census_of(&["a/x", "b/y"]);
    assert_eq!(problems(&real, &census_of(&[]), &["b/y", "a/x"]).len(), 1);
    assert_eq!(
        problems(&real, &census_of(&[]), &["a/x", "a/x", "b/y"]).len(),
        1
    );
    assert_eq!(listed("# note\n\n a/x \nb/y\n"), ["a/x", "b/y"]);
}

/// A scratch botocore data directory holding `files` (path, JSON).
fn scratch(name: &str, files: &[(&str, &str)]) -> Result<PathBuf> {
    let root = std::env::temp_dir().join(format!("fake-matrix-{name}-{}", std::process::id()));
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(path, text)?;
    }
    Ok(root)
}

fn census_at(root: &Path) -> Result<Census> {
    let found = census(root, &ignored()?);
    std::fs::remove_dir_all(root)?;
    found
}

/// Evasion: a fake that botocore cannot load still counting as coverage.
/// A file that does not parse fails the census.
#[test]
fn an_unloadable_fake_fails_the_census() -> Result<()> {
    let root = scratch("broken", &[("zbad/2020-01-01/service-2.json", "{")])?;
    assert!(census_at(&root).is_err());
    Ok(())
}

/// Evasion: features parked in an older API version of a fake, which
/// botocore never loads. Only the latest version counts, as for botocore.
#[test]
fn only_the_latest_version_counts() -> Result<()> {
    let old = r#"{"metadata": {"protocol": "rest-xml"}, "operations": {}, "shapes": {}}"#;
    let new = r#"{"metadata": {"protocol": "json"}, "operations": {}, "shapes": {}}"#;
    let root = scratch(
        "versions",
        &[
            ("zsvc/2019-01-01/service-2.json", old),
            ("zsvc/2020-01-01/service-2.json", new),
        ],
    )?;
    let found = census_at(&root)?;
    assert!(found.contains_key("meta/protocol/json"));
    assert!(!found.contains_key("meta/protocol/rest-xml"));
    Ok(())
}

/// Evasion: a paginators or waiters file for a service that has no
/// service model, which botocore never loads.
#[test]
fn files_of_a_service_without_a_model_are_no_features() -> Result<()> {
    let waiters = r#"{"version": 2, "waiters": {"W": {"acceptors": []}}}"#;
    let root = scratch("orphan", &[("zsvc/2020-01-01/waiters-2.json", waiters)])?;
    assert!(census_at(&root)?.is_empty());
    Ok(())
}
