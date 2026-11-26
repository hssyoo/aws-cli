use super::census::{Census, Error, Features, Ignored, census, defined_list, load, string_list};
use super::forms::{expression_forms, paginator_features, rule_set_features, waiter_features};
use super::matrix::{listed, problems, summary};
use super::observe::{Row, fakes, kebab, mapped, observation_problems, reached, rows};
use super::walk::{PROTOCOL_PRIORITY, resolved_protocol, service_features};
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Error>;
/// FNV-1a 64 of `gaps-baseline.txt` as B7a froze it. Changing it needs an
/// approved card.
const BASELINE_FNV: u64 = 0xd1cf_3ef5_bf7c_efca;
type Reach = std::result::Result<Features, String>;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ignored() -> Result<Ignored> {
    Ignored::read(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"))
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
    let matrix = repository().join("crates/model/tests/fake_matrix");
    let text = std::fs::read_to_string(matrix.join("gaps.txt"))?;
    let baseline = std::fs::read_to_string(matrix.join("gaps-baseline.txt"))?;
    let gaps = listed(&text);
    let table = summary(&real, &fakes, &gaps);
    std::io::Write::write_all(&mut std::io::stderr(), table.as_bytes())?;
    assert!(
        real.len() > 400 && fakes.len() > 100,
        "the census found too little"
    );
    let found = problems(&real, &fakes, &gaps, &listed(&baseline));
    assert!(
        found.is_empty(),
        "{} problems:\n{}",
        found.len(),
        found.join("\n")
    );
    Ok(())
}

#[test]
fn the_ignored_lists_are_found_in_the_model_sources() -> Result<()> {
    let ignored = ignored()?;
    assert!(ignored.operation.contains("readonly") && ignored.operation.contains("documentation"));
    assert_eq!(ignored.http, BTreeSet::from(["responseCode".into()]));
    Ok(())
}

/// Evasion: a renamed, emptied, unclosed or doubly defined list would make
/// every key a feature, or none; the census refuses to run instead. A
/// quoted word in a comment inside a list is no key.
#[test]
fn a_list_must_be_defined_once_and_hold_keys() -> Result<()> {
    let one = |text: &str| vec![(text.to_owned(), PathBuf::from("a.rs"))];
    let list = "const A: &[&str] = &[\"x\", // not \"y\"\n];";
    assert_eq!(defined_list(&one(list), "A")?, BTreeSet::from(["x".into()]));
    assert!(defined_list(&one(list), "B").is_err());
    assert!(defined_list(&[one(list), one(list)].concat(), "A").is_err());
    assert!(string_list(&format!("{list}\n{list}"), "A").is_err());
    assert!(string_list("const A: &[&str] = &[];", "A").is_err());
    assert!(string_list("const A: &[&str] = &[\"x\"", "A").is_err());
    Ok(())
}

/// Evasion: filing a model's features under `metadata.protocol` when
/// botocore speaks another of its `protocols`.
#[test]
fn the_protocol_is_the_one_botocore_resolves() -> Result<()> {
    let both =
        json!({"protocol": "smithy-rpc-v2-cbor", "protocols": ["smithy-rpc-v2-cbor", "json"]});
    assert_eq!(resolved_protocol(Some(&both)), "json");
    assert_eq!(
        resolved_protocol(Some(&json!({"protocol": "query"}))),
        "query"
    );
    assert_eq!(
        resolved_protocol(Some(&json!({"protocols": ["x"]}))),
        "unsupported"
    );
    let ignored = ignored()?;
    let mut model = model(json!({"shape": "Text"}));
    model["metadata"] = both;
    let found = features(|out| service_features(&model, &ignored, out));
    assert!(found.contains("member/json/in/string/body"));
    assert!(
        !found
            .iter()
            .any(|feature| feature.starts_with("member/smithy"))
    );
    Ok(())
}

/// The priority order is botocore's, read from the vendored source.
#[test]
fn the_protocol_priority_is_botocore_s() -> Result<()> {
    let source = std::fs::read_to_string(repository().join("aws-cli/awscli/botocore/utils.py"))?;
    let start = source
        .find("PRIORITY_ORDERED_SUPPORTED_PROTOCOLS = (")
        .ok_or("no tuple")?;
    let end = start + source[start..].find(')').ok_or("unclosed")?;
    let names: Vec<&str> = source[start..end].split('\'').skip(1).step_by(2).collect();
    assert_eq!(names, PROTOCOL_PRIORITY);
    Ok(())
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
        "shape/json/err/string",
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
    let found = problems(&real, &census_of(&["a/x"]), &[], &[]);
    assert_eq!(found.len(), 1);
    assert!(found[0].starts_with("feature b/y is used by 1 shipped models (e.g. svc)"));
    assert!(problems(&real, &census_of(&["a/x"]), &["b/y"], &["b/y"]).is_empty());
}

/// Evasion: a gap left listed after a fake covers it would let the fake
/// regress unseen; it fails as stale.
#[test]
fn a_covered_gap_is_stale() {
    let found = problems(
        &census_of(&["a/x"]),
        &census_of(&["a/x"]),
        &["a/x"],
        &["a/x"],
    );
    assert_eq!(
        found,
        ["gaps.txt line a/x is stale: a fake now exercises it; delete the line"]
    );
}

/// Evasion: listing features no model uses yet, to pre-allow future gaps.
#[test]
fn a_gap_no_shipped_model_uses_is_stale() {
    let found = problems(
        &census_of(&[]),
        &census_of(&[]),
        &["future/trait"],
        &["future/trait"],
    );
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("no shipped model uses it"));
}

/// Evasion: a repeated or unsorted line hides a second copy of an entry
/// from review.
#[test]
fn gaps_must_be_sorted_and_unique() {
    let real = census_of(&["a/x", "b/y"]);
    assert_eq!(
        problems(&real, &census_of(&[]), &["b/y", "a/x"], &["b/y", "a/x"]).len(),
        1
    );
    assert_eq!(
        problems(
            &real,
            &census_of(&[]),
            &["a/x", "a/x", "b/y"],
            &["a/x", "a/x", "b/y"]
        )
        .len(),
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

/// A file that is not JSON fails the census rather than counting as
/// coverage. (That botocore loads each fake is the corpus rows' check.)
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

/// Evasion: un-faking a feature and listing it as a gap. A line not in the
/// frozen baseline fails, so the list cannot grow.
#[test]
fn a_gap_outside_the_baseline_fails() {
    let real = census_of(&["a/x", "b/y"]);
    let found = problems(&real, &census_of(&["a/x"]), &["b/y"], &["a/x"]);
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("not in gaps-baseline.txt"));
}

/// The extras file's `merge` is deep-merged, as botocore's loader does,
/// and paginators come from the service model's version, not their own
/// latest.
#[test]
fn extras_merge_and_files_follow_the_service_version() -> Result<()> {
    let model = r#"{"metadata": {"protocol": "json"}, "operations": {}, "shapes": {}}"#;
    let extras = r#"{"version": 1, "merge": {"metadata": {"signingName": "z"}}}"#;
    let pages = r#"{"pagination": {"L": {"more_results": "a.b"}}}"#;
    let root = scratch(
        "extras",
        &[
            ("zsvc/2020-01-01/service-2.json", model),
            ("zsvc/2020-01-01/service-2.sdk-extras.json", extras),
            ("zsvc/2099-01-01/paginators-1.json", pages),
        ],
    )?;
    let merged = load(&root.join("zsvc/2020-01-01"), "service-2")?.ok_or("no model")?;
    assert_eq!(merged["metadata"]["signingName"], "z");
    let found = census_at(&root)?;
    assert!(found.contains_key("meta/signingName"));
    assert!(!found.keys().any(|feature| feature.starts_with("pages/")));
    Ok(())
}

/// Evasion: one plain waiter path covering every JMESPath form, or a
/// parameter's name standing in for its path's form.
#[test]
fn expression_forms_are_named() {
    let forms = expression_forms("length(xs[?a == 'b' || !c]) > `0` && ys[].z");
    for form in [
        "or", "and", "not", "compare", "literal", "filter", "flatten", "dotted",
    ] {
        assert!(
            forms.iter().any(|found| found == form),
            "{form} not in {forms:?}"
        );
    }
    assert!(forms.iter().any(|found| found == "fn-length"));
    assert_eq!(expression_forms("Items"), ["field"]);
    assert_eq!(expression_forms("keys(RequestItems)"), ["fn-keys"]);
    assert!(expression_forms("T[*].[A.B, C.D][]").contains(&"multiselect".to_owned()));
    assert!(expression_forms("resourceId[0]").contains(&"index".to_owned()));
}

/// Evasion: an output member covering a modeled-error one, a format's
/// presence covering each format, a trait object covering its keys, and a
/// body list covering a header list's elements.
#[test]
fn errors_values_and_header_elements_are_their_own_features() -> Result<()> {
    let ignored = ignored()?;
    let mut json = model(json!({"shape": "Names", "location": "header"}));
    json["metadata"]["protocol"] = json!("rest-json");
    let shapes = &mut json["shapes"];
    shapes["Names"] = json!({"type": "list", "member": {"shape": "When"}});
    shapes["When"]["timestampFormat"] = json!("iso8601");
    shapes["Busy"] = json!({"type": "structure", "members": {}, "exception": true,
                            "retryable": {"throttling": true}, "error": {"httpStatusCode": 429}});
    json["operations"]["Put"]["errors"] = json!([{"shape": "Busy"}]);
    let found = features(|out| service_features(&json, &ignored, out));
    for feature in [
        "member/rest-json/in/list/header/member/timestamp",
        "shape/rest-json/in/timestamp/timestampFormat/iso8601",
        "shape/rest-json/err/structure/exception",
        "shape/rest-json/err/structure/error/httpStatusCode",
        "trait/structure/retryable/throttling",
    ] {
        assert!(found.contains(feature), "{feature} not in {found:?}");
    }
    assert!(
        !found
            .iter()
            .any(|feature| feature.starts_with("shape/rest-json/out/"))
    );
    Ok(())
}

fn read(path: PathBuf) -> Result<String> {
    std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()).into())
}

/// Each row's reached features, keyed by id.
fn reach_of(models: &Path, rows: &[Row]) -> Result<BTreeMap<String, Reach>> {
    let (fakes, ignored) = (fakes(models)?, ignored()?);
    let reach = rows
        .iter()
        .map(|row| (row.id.clone(), reached(&fakes, &ignored, row)));
    Ok(reach.collect())
}

/// G-039: every closed gap line is reached by a corpus row that
/// `observed.txt` names.
#[test]
fn every_closed_gap_line_is_observed_by_a_corpus_row() -> Result<()> {
    let matrix = repository().join("crates/model/tests/fake_matrix");
    let (baseline, gaps) = (
        read(matrix.join("gaps-baseline.txt"))?,
        read(matrix.join("gaps.txt"))?,
    );
    let open: BTreeSet<&str> = listed(&gaps).into_iter().collect();
    let closed: BTreeSet<&str> = listed(&baseline)
        .into_iter()
        .filter(|line| !open.contains(line))
        .collect();
    let rows = rows(&repository().join("tests/diff/corpus"))?;
    let reach = reach_of(&repository().join("tests/models"), &rows)?;
    let text = read(matrix.join("observed.txt"))?;
    let found = observation_problems(&closed, &mapped(&text)?, &reach);
    assert!(
        found.is_empty(),
        "{} problems:\n{}",
        found.len(),
        found.join("\n")
    );
    Ok(())
}

#[test]
fn kebab_is_botocore_s_xform_name() {
    for (name, command) in [
        ("GetWidget", "get-widget"),
        ("GetWidgetV2", "get-widget-v2"),
        ("PutS3Object", "put-s3-object"),
        ("DescribeDBInstances", "describe-db-instances"),
        ("WidgetExists", "widget-exists"),
        ("ListACLs", "list-acls"),
        ("ListAWSAccounts", "list-aws-accounts"),
        ("GetAPIKeys", "get-api-keys"),
        ("Get2FAStatus", "get2-fa-status"),
    ] {
        assert_eq!(kebab(name), command);
    }
}

/// A json fake with `Shown` (a plain input) and `Hidden` (an input with an
/// idempotency token), a paginator and a waiter on `Shown`.
fn seeded_fake(name: &str) -> Result<PathBuf> {
    let model = json!({"metadata": {"protocol": "json"}, "operations": {
        "Shown": {"name": "Shown", "http": {"method": "POST", "requestUri": "/"},
                  "input": {"shape": "Plain"}, "output": {"shape": "Plain"},
                  "errors": [{"shape": "Missing"}]},
        "Hidden": {"name": "Hidden", "http": {"method": "POST", "requestUri": "/"},
                   "input": {"shape": "Tokened"}}},
        "shapes": {"Text": {"type": "string"},
                   "Plain": {"type": "structure", "members": {"Next": {"shape": "Text"}}},
                   "Missing": {"type": "structure", "exception": true,
                               "members": {"Reason": {"shape": "Text"}}},
                   "Tokened": {"type": "structure", "members": {
                       "Token": {"shape": "Text", "idempotencyToken": true}}}}});
    let pages = json!({"pagination": {"Shown": {"input_token": "Next", "output_token": "Next"}}});
    let waiters = json!({"version": 2, "waiters": {"ShownReady": {"operation": "Shown",
        "delay": 1, "maxAttempts": 2, "acceptors": [{"matcher": "status", "state": "success",
        "expected": 200}]}}});
    let files = [
        ("zseen/2020-01-01/service-2.json", model.to_string()),
        ("zseen/2020-01-01/paginators-1.json", pages.to_string()),
        ("zseen/2020-01-01/waiters-2.json", waiters.to_string()),
    ];
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (*path, text.as_str()))
        .collect();
    scratch(name, &files)
}

fn row(id: &str, argv: &[&str]) -> Row {
    replying(id, argv, &[200])
}

fn replying(id: &str, argv: &[&str], statuses: &[u64]) -> Row {
    let argv = argv.iter().map(|argument| (*argument).to_owned()).collect();
    Row {
        id: id.into(),
        argv,
        statuses: statuses.to_vec(),
    }
}

/// The seeded defect: a fake declares a feature on an operation no row
/// runs. The check fails, naming it, whether the map names a row that
/// runs another operation or names none.
#[test]
fn a_feature_no_row_reaches_fails_the_observation_check() -> Result<()> {
    let root = seeded_fake("observe-seeded")?;
    let rows = [row("shown", &["zseen", "shown"])];
    let reach = reach_of(&root, &rows);
    std::fs::remove_dir_all(&root)?;
    let reach = reach?;
    let hidden = "member/json/in/string/body/idempotencyToken";
    let closed = BTreeSet::from([hidden]);
    let found = observation_problems(&closed, &[(hidden, "shown")], &reach);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[1].starts_with(&format!("gap line {hidden} is closed")));
    assert!(
        found[0].contains("row shown does not reach member/json/in/string/body/idempotencyToken")
    );
    let found = observation_problems(&closed, &[], &reach);
    assert!(
        found[0].starts_with(&format!("gap line {hidden} is closed")),
        "{found:?}"
    );
    assert!(found[0].contains("rows that reach it: []"));
    let found = observation_problems(&closed, &[(hidden, "ghost")], &reach);
    assert_eq!(found[0], "observed.txt: row ghost is in no corpus file");
    assert!(found[1].starts_with(&format!("gap line {hidden} is closed")));
    Ok(())
}

/// Evasion: a repeated or unsorted map line hides an entry from review,
/// and a line that is not `feature row-id` is refused.
#[test]
fn map_lines_are_sorted_unique_and_well_formed() -> Result<()> {
    let reach = BTreeMap::from([("r".to_owned(), Ok(Features::from(["a/x".to_owned()])))]);
    let closed = BTreeSet::from(["a/x"]);
    let twice = observation_problems(&closed, &[("a/x", "r"), ("a/x", "r")], &reach);
    assert_eq!(
        twice,
        ["observed.txt line a/x r is out of order or repeated"]
    );
    assert!(mapped("# c\na/x r\n")? == [("a/x", "r")] && mapped("a/x").is_err());
    Ok(())
}

/// A row reaches its operation's paginator and the service's features,
/// a `wait` row its waiter and the waiter's operation, and a row on an
/// unknown command or service reaches nothing.
#[test]
fn rows_reach_their_paginator_waiter_and_service() -> Result<()> {
    let root = seeded_fake("observe-reach")?;
    let rows = [
        row("pages", &["zseen", "shown"]),
        row("wait", &["zseen", "wait", "shown-ready"]),
        row("typo", &["zseen", "showen"]),
        row("other", &["zother", "shown"]),
    ];
    let reach = reach_of(&root, &rows);
    std::fs::remove_dir_all(&root)?;
    let reach = reach?;
    let pages = reach["pages"].clone()?;
    assert!(pages.contains("pages/input_token/string") && pages.contains("meta/protocol/json"));
    assert!(
        !pages
            .iter()
            .any(|feature| feature.contains("idempotencyToken"))
    );
    let wait = reach["wait"].clone()?;
    assert!(wait.contains("wait/status/success") && wait.contains("op/json/input"));
    assert!(
        reach["typo"]
            .as_ref()
            .is_err_and(|why| why.contains("`showen`"))
    );
    assert!(
        reach["other"]
            .as_ref()
            .is_err_and(|why| why.contains("no fake"))
    );
    let closed = BTreeSet::from(["pages/input_token/string"]);
    let stale = observation_problems(&closed, &[("meta/protocol/json", "pages")], &reach);
    assert!(stale.iter().any(|problem| problem.contains("is stale")));
    Ok(())
}

/// M1 (rev-b7c): a row observes output features only with a success
/// reply and error features only with an error reply; S2: a top-level
/// input member only when the row passes its flag, or the CLI fills it.
#[test]
fn a_row_reaches_only_its_reply_kinds_and_the_members_it_sends() -> Result<()> {
    let root = seeded_fake("observe-direction")?;
    let rows = [
        replying("ok", &["zseen", "shown"], &[200]),
        replying("error", &["zseen", "shown"], &[404]),
        replying("next", &["zseen", "shown", "--next", "n"], &[200]),
        replying("filled", &["zseen", "hidden"], &[200]),
    ];
    let reach = reach_of(&root, &rows);
    std::fs::remove_dir_all(&root)?;
    let reach = reach?;
    let (err, out, sent) = (
        "shape/json/err/structure/exception",
        "member/json/out/string/body",
        "member/json/in/string/body",
    );
    let (ok, error) = (reach["ok"].clone()?, reach["error"].clone()?);
    assert!(!ok.contains(err) && ok.contains(out) && !ok.contains(sent));
    assert!(error.contains(err) && !error.contains(out) && !error.contains("op/json/output"));
    assert!(reach["next"].clone()?.contains(sent));
    assert!(
        reach["filled"]
            .clone()?
            .contains("member/json/in/string/body/idempotencyToken")
    );
    let found = observation_problems(&BTreeSet::from([err]), &[(err, "ok")], &reach);
    assert!(found[0].contains("row ok does not reach shape/json/err/structure/exception"));
    Ok(())
}

/// The shipped map against the corpus with some rows removed.
fn problems_without(removed: impl Fn(&Row) -> bool, rename: (&str, &str)) -> Result<Vec<String>> {
    let matrix = repository().join("crates/model/tests/fake_matrix");
    let (baseline, gaps) = (
        read(matrix.join("gaps-baseline.txt"))?,
        read(matrix.join("gaps.txt"))?,
    );
    let open: BTreeSet<&str> = listed(&gaps).into_iter().collect();
    let closed: BTreeSet<&str> = listed(&baseline)
        .into_iter()
        .filter(|line| !open.contains(line))
        .collect();
    let mut rows = rows(&repository().join("tests/diff/corpus"))?;
    rows.retain(|row| !removed(row));
    let reach = reach_of(&repository().join("tests/models"), &rows)?;
    let text = read(matrix.join("observed.txt"))?;
    let mut map = mapped(&text)?;
    for entry in &mut map {
        if entry.1 == rename.0 {
            entry.1 = rename.1;
        }
    }
    map.sort();
    Ok(observation_problems(&closed, &map, &reach))
}

/// rev-b7c M1's deletion: without the rows that get an error reply, every
/// error-path line loses its observer.
#[test]
fn removing_the_error_rows_fails_every_error_line() -> Result<()> {
    let found = problems_without(
        |row| row.statuses.iter().any(|status| *status >= 300),
        ("", ""),
    )?;
    let error_lines = found
        .iter()
        .filter(|problem| problem.starts_with("gap line") && problem.contains("/err/"))
        .count();
    assert!(
        error_lines >= 50,
        "{} error-line problems:\n{}",
        error_lines,
        found.join("\n")
    );
    Ok(())
}

/// rev-b7c S2's deletion: the full put-gadget row removed and its entries
/// moved to the minimal row, which passes none of those members' flags.
#[test]
fn a_minimal_row_cannot_stand_in_for_the_members_it_omits() -> Result<()> {
    let full = "fakes-zrestjson2-put-gadget";
    let found = problems_without(
        |row| row.id == full,
        (full, "fakes-zrestjson2-put-gadget-minimal"),
    )?;
    let unsent = found
        .iter()
        .filter(|problem| problem.contains("does not reach member/rest-json/in"));
    assert!(unsent.count() >= 20, "{}", found.join("\n"));
    Ok(())
}

/// S1 (rev-b7c): the frozen baseline the closed set comes from is pinned,
/// so deleting a line from it and from gaps.txt cannot close the line.
#[test]
fn the_gap_baseline_is_pinned() -> Result<()> {
    let text = read(repository().join("crates/model/tests/fake_matrix/gaps-baseline.txt"))?;
    let fnv = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    assert_eq!(
        (listed(&text).len(), fnv),
        (730, BASELINE_FNV),
        "gaps-baseline.txt changed"
    );
    Ok(())
}
