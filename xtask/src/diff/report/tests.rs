use std::path::PathBuf;

use super::*;
use crate::diff::capture::Request;
use crate::diff::corpus;
use crate::diff::options::Route;

fn side(route: Route) -> Side {
    Side {
        label: "candidate".to_owned(),
        binary: PathBuf::from("aws"),
        route,
    }
}

fn run(b: Route) -> Run {
    Run {
        a: side(Route::Native),
        b: side(b),
        corpus: vec![PathBuf::from("c.jsonl")],
        filter: None,
        known: None,
        out: PathBuf::new(),
    }
}

fn rows(ids: &[&str]) -> Result<Vec<Row>, TaskError> {
    let lines: Vec<String> = ids
        .iter()
        .map(|id| format!("{{\"id\":\"{id}\",\"argv\":[\"x\"],\"responses\":[]}}"))
        .collect();
    corpus::parse("rows", &lines.join("\n"))
}

/// A run that printed `stdout` and sent one request with `target`.
fn capture(id: &str, stdout: &str, target: &str, date: &str) -> Capture {
    let header = |name: &str, value: &str| (name.to_owned(), value.to_owned());
    Capture {
        id: id.to_owned(),
        side: String::new(),
        exit: Some(0),
        timed_out: false,
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
        requests: vec![Request {
            method: "POST".to_owned(),
            target: "/".to_owned(),
            version: "1.1".to_owned(),
            headers: vec![header("X-Amz-Target", target), header("X-Amz-Date", date)],
            body: b"{}".to_vec(),
            complete: true,
        }],
    }
}

const KNOWN: &str = r#"[
  {"fingerprint": "stdout:$.A", "cases": ["known"], "reason": "a documented difference"},
  {"fingerprint": "stdout:$.A", "cases": "*", "mode": "oracle", "reason": "only between routes"},
  {"fingerprint": "exit:0->1", "cases": "*", "reason": "never seen"}
]"#;

/// Rows `same`, `known` and `new`: the first identical but for the
/// normalized date, the others differing in stdout and in a header.
fn report(b: Route) -> Result<Report, TaskError> {
    let rows = rows(&["same", "known", "new"])?;
    let pairs = vec![
        (
            capture("same", "{}", "T.A", "1"),
            capture("same", "{}", "T.A", "2"),
        ),
        (
            capture("known", "{\"A\": 1}", "T.A", "1"),
            capture("known", "{\"A\": 2}", "T.A", "1"),
        ),
        (
            capture("new", "{\"A\": 1}", "T.A", "1"),
            capture("new", "{\"A\": 2}", "T.B", "1"),
        ),
    ];
    Ok(build(&run(b), &rows, &pairs, &known(KNOWN)?))
}

#[test]
fn rows_are_identical_known_or_mismatched() -> Result<(), TaskError> {
    let report = report(Route::Native)?;
    assert_eq!(
        (
            report.rows,
            report.identical,
            report.known,
            report.mismatched
        ),
        (3, 1, 1, 1)
    );
    assert_eq!(
        report.fingerprints(),
        ["stdout:$.A", "request[*].header:x-amz-target:value"]
    );
    let groups: Vec<_> = report
        .groups
        .iter()
        .map(|group| (group.known, group.instances.len()))
        .collect();
    assert_eq!(groups, [(false, 2), (false, 1)]);
    let proposed: Vec<_> = report
        .proposed
        .iter()
        .map(|p| (p.fingerprint.as_str(), p.cases.clone()))
        .collect();
    assert_eq!(
        proposed,
        [
            ("stdout:$.A", vec!["new".to_owned()]),
            (
                "request[*].header:x-amz-target:value",
                vec!["new".to_owned()]
            ),
        ]
    );
    assert_eq!(report.unused_known, ["stdout:$.A", "exit:0->1"]);
    assert_eq!(
        report
            .verdict()
            .err()
            .map(|error| error.to_string())
            .as_deref(),
        Some("1 of 3 rows differ outside the known differences")
    );
    let summary = report.summary();
    assert!(
        summary.contains("diff: 3 rows: 1 identical, 1 known, 1 mismatched\n"),
        "{summary}"
    );
    assert!(
        summary.contains(
            "     1 NEW   request[*].header:x-amz-target:value  new: \"T.A\" vs \"T.B\"\n"
        ),
        "{summary}"
    );
    Ok(())
}

#[test]
fn an_entry_with_a_mode_holds_only_in_it() -> Result<(), TaskError> {
    let report = report(Route::Python)?;
    assert_eq!(report.mode, "oracle");
    assert_eq!(
        (report.identical, report.known, report.mismatched),
        (1, 1, 1)
    );
    assert_eq!(
        report.fingerprints(),
        ["request[*].header:x-amz-target:value", "stdout:$.A"]
    );
    assert_eq!(report.groups.last().map(|group| group.known), Some(true));
    assert_eq!(report.unused_known, ["exit:0->1"]);
    Ok(())
}

#[test]
fn the_known_differences_need_cases_and_a_reason() {
    let failure = |text: &str| {
        known(text)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default()
    };
    assert_eq!(known("[]").ok(), Some(Vec::new()));
    assert_eq!(
        failure(r#"[{"fingerprint": "f", "cases": "*", "reason": " "}]"#),
        "the known difference f gives no reason"
    );
    assert_eq!(
        failure(r#"[{"fingerprint": "f", "cases": "*", "mode": "route", "reason": "r"}]"#),
        "the known difference f has mode \"route\", not one of oracle, baseline, determinism"
    );
    for text in [
        r#"[{"fingerprint": "f", "cases": "all", "reason": "r"}]"#,
        r#"[{"fingerprint": "f", "reason": "r"}]"#,
        r#"[{"fingerprint": "f", "cases": "*", "reason": "r", "why": ""}]"#,
    ] {
        assert!(failure(text).starts_with("known differences: "), "{text}");
    }
}

#[test]
fn the_report_is_json() -> Result<(), TaskError> {
    let value: serde_json::Value = serde_json::from_str(&report(Route::Native)?.json()?)
        .map_err(|error| deps::error(error.to_string()))?;
    assert_eq!(value["format"], 1);
    assert_eq!(value["mode"], "determinism");
    assert_eq!(value["a"]["side"], "candidate:native");
    assert_eq!(value["groups"][0]["instances"][0]["id"], "known");
    assert_eq!(value["groups"][0]["instances"][0]["known"], true);
    assert_eq!(value["proposed"][0]["reason"], "");
    Ok(())
}
