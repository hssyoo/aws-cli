use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::diff::capture;
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

/// Rows that declare they send no request.
fn silent(ids: &[&str]) -> Result<Vec<Row>, TaskError> {
    let mut rows = rows(ids)?;
    rows.iter_mut().for_each(|row| row.requests = Some(0));
    Ok(rows)
}

/// A request as the capture writes it.
fn request(target: &str, headers: &[(&str, &str)], body: &str) -> Value {
    let headers: Vec<[&str; 2]> = headers
        .iter()
        .map(|(name, value)| [*name, *value])
        .collect();
    json!({"method": "POST", "target": target, "version": "1.1", "headers": headers,
           "body": body, "complete": true})
}

/// A run as the capture writes it, with `fields` over a run that exits 0
/// and prints and sends nothing.
fn run_of(id: &str, fields: Value) -> Result<Capture, TaskError> {
    let mut value = json!({"id": id, "side": "a", "exit": 0, "timed_out": false,
                           "stdout": "", "stderr": "", "requests": []});
    if let (Some(value), Some(fields)) = (value.as_object_mut(), fields.as_object()) {
        value.extend(fields.clone());
    }
    capture::parse(&value.to_string())?
        .pop()
        .ok_or_else(|| deps::error("no capture"))
}

/// A run that printed `stdout` and sent one request with `target`.
fn capture(id: &str, stdout: &str, target: &str, date: &str) -> Result<Capture, TaskError> {
    let headers = [("X-Amz-Target", target), ("X-Amz-Date", date)];
    run_of(
        id,
        json!({"stdout": stdout, "requests": [request("/", &headers, "{}")]}),
    )
}

const KNOWN: &str = r#"[
  {"fingerprint": "stdout:$.A", "cases": ["known"], "mode": "determinism", "reason": "documented"},
  {"fingerprint": "stdout:$.A", "cases": "*", "mode": "oracle", "reason": "only between routes"},
  {"fingerprint": "exit:0->1", "cases": "*", "mode": "determinism", "reason": "never seen"}
]"#;

const DATE: &str = "20261125T120000Z";

/// Rows `same`, `known` and `new`: the first identical but for the
/// normalized date, the others differing in stdout and in a header.
fn report(b: Route) -> Result<Report, TaskError> {
    let rows = rows(&["same", "known", "new"])?;
    let pairs = vec![
        (
            capture("same", "{}", "T.A", DATE)?,
            capture("same", "{}", "T.A", "20261125T120001Z")?,
        ),
        (
            capture("known", "{\"A\": 1}", "T.A", DATE)?,
            capture("known", "{\"A\": 2}", "T.A", DATE)?,
        ),
        (
            capture("new", "{\"A\": 1}", "T.A", DATE)?,
            capture("new", "{\"A\": 2}", "T.B", DATE)?,
        ),
    ];
    Ok(build(&run(b), &rows, &pairs, &known(KNOWN)?))
}

fn fingerprints(report: &Report) -> Vec<&str> {
    report
        .groups
        .iter()
        .map(|group| group.fingerprint.as_str())
        .collect()
}

#[test]
fn rows_are_identical_known_or_mismatched() -> Result<(), TaskError> {
    let report = report(Route::Native)?;
    let counts = (
        report.rows,
        report.identical,
        report.known,
        report.mismatched,
    );
    assert_eq!(counts, (3, 1, 1, 1));
    assert_eq!(
        fingerprints(&report),
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
    let new = vec!["new".to_owned()];
    assert_eq!(
        proposed,
        [
            ("stdout:$.A", new.clone()),
            ("request[*].header:x-amz-target:value", new)
        ]
    );
    assert_eq!(report.unused_known, ["stdout:$.A", "exit:0->1"]);
    Ok(())
}

#[test]
fn the_verdict_and_summary_follow_the_rows() -> Result<(), TaskError> {
    let report = report(Route::Native)?;
    let rows = report.by_row();
    assert_eq!(rows.get("new").map(Vec::len), Some(2));
    assert_eq!(rows.get("same"), None);
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
        summary.contains("diff: 3 rows: 1 identical, 1 known, 1 mismatched, 0 not native\n"),
        "{summary}"
    );
    let line = "     1 NEW   request[*].header:x-amz-target:value  new: \"T.A\" vs \"T.B\"\n";
    assert!(summary.contains(line), "{summary}");
    Ok(())
}

#[test]
fn an_entry_holds_only_in_its_mode() -> Result<(), TaskError> {
    let report = report(Route::Python)?;
    assert_eq!(report.mode, "oracle");
    assert_eq!(
        (report.identical, report.known, report.mismatched),
        (1, 1, 1)
    );
    assert_eq!(
        fingerprints(&report),
        ["request[*].header:x-amz-target:value", "stdout:$.A"]
    );
    assert_eq!(report.groups.last().map(|group| group.known), Some(true));
    assert_eq!(report.unused_known, ["stdout:$.A", "exit:0->1"]);
    Ok(())
}

#[test]
fn the_known_differences_need_cases_a_mode_and_a_reason() {
    let failure = |text: &str| {
        known(text)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default()
    };
    assert_eq!(known("[]").ok(), Some(Vec::new()));
    assert_eq!(
        failure(r#"[{"fingerprint": "f", "cases": "*", "mode": "oracle", "reason": " "}]"#),
        "the known difference f gives no reason"
    );
    assert_eq!(
        failure(r#"[{"fingerprint": "f", "cases": "*", "mode": "route", "reason": "r"}]"#),
        "the known difference f has mode \"route\", not one of oracle, baseline, determinism"
    );
    for text in [
        r#"[{"fingerprint": "f", "cases": "all", "mode": "oracle", "reason": "r"}]"#,
        r#"[{"fingerprint": "f", "mode": "oracle", "reason": "r"}]"#,
        r#"[{"fingerprint": "f", "cases": "*", "reason": "r"}]"#,
        r#"[{"fingerprint": "f", "cases": "*", "mode": "oracle", "reason": "r", "why": ""}]"#,
    ] {
        assert!(failure(text).starts_with("known differences: "), "{text}");
    }
}

#[test]
fn the_report_is_json() -> Result<(), TaskError> {
    let value: Value = serde_json::from_str(&report(Route::Native)?.json()?)
        .map_err(|error| deps::error(error.to_string()))?;
    assert_eq!(value["format"], 1);
    assert_eq!(value["mode"], "determinism");
    assert_eq!(value["a"]["side"], "candidate:native");
    assert_eq!(value["groups"][0]["instances"][0]["id"], "known");
    assert_eq!(value["groups"][0]["instances"][0]["known"], true);
    assert_eq!(value["proposed"][0]["reason"], "");
    Ok(())
}

// The countermeasures for gap G-018: each test fails on 22426dbf3.

/// The fingerprints of row `r`'s runs `a` and `b`.
fn named(a: &Capture, b: &Capture) -> Result<Vec<String>, TaskError> {
    let rows = rows(&["r"])?;
    let row = rows.first().ok_or_else(|| deps::error("no row"))?;
    Ok(row_diffs(row, a, b)
        .into_iter()
        .map(|diff| diff.fingerprint)
        .collect())
}

/// `text` with each byte in turn doubled, deleted, and flipped in its
/// lowest bit.
fn mutations(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    for at in 0..bytes.len() {
        let (mut doubled, mut deleted, mut flipped) =
            (bytes.to_vec(), bytes.to_vec(), bytes.to_vec());
        doubled.insert(at, bytes[at]);
        deleted.remove(at);
        flipped[at] ^= 1;
        found.extend(
            [doubled, deleted, flipped].map(|text| String::from_utf8_lossy(&text).into_owned()),
        );
    }
    found
}

/// What builds a run's fields from one text.
type Field<'a> = dyn Fn(&str) -> Value + 'a;

#[test]
fn every_byte_outside_the_normalized_spans_shows() -> Result<(), TaskError> {
    let host = [("Host", "zjson.us-west-2.amazonaws.com")];
    let form = [
        ("Host", "h"),
        ("Content-Type", "application/x-www-form-urlencoded"),
    ];
    let target = |target: &str| json!({"requests": [request(target, &host, "")]});
    let json_body = |body: &str| json!({"requests": [request("/", &host, body)]});
    let form_body = |body: &str| json!({"requests": [request("/", &form, body)]});
    let stdout = |stdout: &str| json!({"stdout": stdout});
    let cases: [(&Field<'_>, &str); 6] = [
        (&target, "/things/t-1?Action=Get&flag&empty=&x=1"),
        (&json_body, "{\"A\": [1, \"b\"], \"C\": {}}"),
        (&form_body, "Action=Get&Tags.member.1.Key=k&flag"),
        (&json_body, "<R a=\"1\"><B>t</B><C/></R>"),
        (&stdout, "{\n    \"A\": 1,\n    \"B\": \"x\"\n}\n"),
        (&stdout, "first line\nsecond 2\n"),
    ];
    for (field, text) in cases {
        let a = run_of("r", field(text))?;
        for mutated in mutations(text) {
            let found = named(&a, &run_of("r", field(&mutated))?)?;
            assert!(
                !found.is_empty(),
                "{text:?} -> {mutated:?} shows no difference"
            );
        }
    }
    Ok(())
}

/// Asserts each pair of runs, given by their fields, differs by both
/// fingerprints named.
fn both_named(cases: &[(Value, Value, [&str; 2])]) -> Result<(), TaskError> {
    for (a, b, expected) in cases {
        let found = named(&run_of("r", a.clone())?, &run_of("r", b.clone())?)?;
        for fingerprint in expected {
            assert!(
                found.iter().any(|name| name == fingerprint),
                "{fingerprint} not in {found:?}"
            );
        }
    }
    Ok(())
}

fn printing(stdout: &str) -> Value {
    json!({"stdout": stdout})
}

fn sending(headers: &[(&str, &str)], target: &str, body: &str) -> Value {
    json!({"requests": [request(target, headers, body)]})
}

#[test]
fn two_causes_in_the_output_are_two_fingerprints() -> Result<(), TaskError> {
    both_named(&[
        (
            printing("{\"A\": 1, \"B\": \"x\"}"),
            printing("{\"B\": \"y\", \"A\": 1}"),
            ["stdout:$.B", "stdout:$:order"],
        ),
        (
            printing("{\n    \"A\": 1\n}"),
            printing("{\"A\": 2}"),
            ["stdout:$.A", "stdout:json-format"],
        ),
        (
            printing("a\nb 1\nc\nd 2\n"),
            printing("a\nb 5\nc\nd 6 more\n"),
            ["stdout:text:b #->b #", "stdout:text:d #->d # more"],
        ),
    ])
}

#[test]
fn two_causes_in_a_request_are_two_fingerprints() -> Result<(), TaskError> {
    let host = ("Host", "h");
    let form = [host, ("Content-Type", "application/x-www-form-urlencoded")];
    let agent = |value| sending(&[("User-Agent", value)], "/", "");
    both_named(&[
        (
            sending(&[host], "/", "{\"T\": \"t\", \"L\": 1.0}"),
            sending(&[host], "/", "{\"L\": 1, \"T\": \"t\"}"),
            ["request[*].body:$.L", "request[*].body:$:order"],
        ),
        (
            sending(&[host, ("X-T", "a"), ("C", "c")], "/", ""),
            sending(&[("C", "c"), ("X-T", "a"), host, ("Expect", "e")], "/", ""),
            ["request[*].header:expect:extra", "request[*].header-order"],
        ),
        (
            sending(&[("X-M", "1, 2")], "/", ""),
            sending(&[("x-m", "1"), ("x-m", "2")], "/", ""),
            ["request[*].header:x-m:count", "request[*].header:x-m:case"],
        ),
        (
            agent("aws-cli/2 lang/rust#1 ua/2.1 m/E"),
            agent("aws-cli/2 m/E ua/2.1 lang/python#3"),
            [
                "request[*].header:user-agent:lang",
                "request[*].header:user-agent:order",
            ],
        ),
        (
            sending(&[host], "/p?a=1&b=2", ""),
            sending(&[host], "/p?b=2&a=9", ""),
            ["request[*].line:query:a", "request[*].line:query-order"],
        ),
        (
            sending(&[host], "/p?k&a=1", ""),
            sending(&[host], "/p?k=&a=2", ""),
            ["request[*].line:query:k", "request[*].line:query:a"],
        ),
        (
            sending(&form, "/", "Action=A&x=1"),
            sending(&form, "/", "&Action=A&x=2"),
            ["request[*].body:x", "request[*].body-format"],
        ),
        (
            sending(&[host], "/", "<R n=\"a\"><B>1</B></R>"),
            sending(&[host], "/", "<R n=\"b\"><B>2</B></R>"),
            ["request[*].body:/R", "request[*].body:/R/B"],
        ),
    ])
}

/// Known differences of one entry for every row.
fn entry(fingerprint: &str, mode: &str) -> String {
    format!(
        r#"[{{"fingerprint": "{fingerprint}", "cases": "*", "mode": "{mode}", "reason": "r"}}]"#
    )
}

#[test]
fn a_known_cause_does_not_cover_another_in_its_stage() -> Result<(), TaskError> {
    let agent = |value: &str| {
        run_of(
            "r",
            json!({"requests": [request("/", &[("User-Agent", value)], "")]}),
        )
    };
    let pairs = [(
        agent("aws-cli/2 lang/rust#1 ua/2.1")?,
        agent("ua/2.1 aws-cli/2 lang/python#3")?,
    )];
    let known: Vec<Known> =
        serde_json::from_str(&entry("request[*].header:user-agent:lang", "oracle"))
            .map_err(|error| deps::error(error.to_string()))?;
    let report = build(&run(Route::Python), &rows(&["r"])?, &pairs, &known);
    assert_eq!((report.known, report.mismatched), (0, 1));
    Ok(())
}

#[test]
fn timeouts_and_harness_errors_are_never_known() -> Result<(), TaskError> {
    let text = entry("timeout:both", "determinism");
    assert!(known(&text).is_err(), "an entry for a timeout was accepted");
    let entries: Vec<Known> =
        serde_json::from_str(&text).map_err(|error| deps::error(error.to_string()))?;
    let hung = || run_of("r", json!({"exit": null, "timed_out": true}));
    let report = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[(hung()?, hung()?)],
        &entries,
    );
    assert_eq!((report.known, report.mismatched), (0, 1));
    let unparsed = json!({"harness": ["protocol_error: a connection closed in request_headers"]});
    let found = named(&run_of("r", unparsed.clone())?, &run_of("r", unparsed)?)?;
    assert_eq!(
        found,
        ["harness:a:protocol_error", "harness:b:protocol_error"]
    );
    let harness = entry("harness:a:protocol_error", "oracle");
    assert!(
        known(&harness).is_err(),
        "an entry for a harness error was accepted"
    );
    Ok(())
}

// B3.2: each test fails on 64fd03d17.

/// get-vanilla of the SigV4 test suite, as a run with its secret key.
fn vanilla(signature: &str) -> Result<Capture, TaskError> {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature={signature}"
    );
    let headers = [
        ("Host", "example.amazonaws.com"),
        ("X-Amz-Date", "20150830T123600Z"),
        ("Authorization", authorization.as_str()),
    ];
    let mut request = request("/", &headers, "");
    request["method"] = json!("GET");
    let secret = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    run_of("r", json!({"secret_key": secret, "requests": [request]}))
}

#[test]
fn a_tampered_signature_is_bad_where_a_good_one_is_ok() -> Result<(), TaskError> {
    let good = "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31";
    let tampered = good.replacen('5', "6", 1);
    let diffs = row_diffs(&rows(&["r"])?[0], &vanilla(good)?, &vanilla(&tampered)?);
    let shown: Vec<_> = diffs
        .iter()
        .map(|diff| {
            let verdict = |excerpt: &str| excerpt.rsplit("Signature=").next().map(str::to_owned);
            (
                diff.fingerprint.as_str(),
                verdict(&diff.a),
                verdict(&diff.b),
            )
        })
        .collect();
    let verdict = |text: &str| Some(text.to_owned());
    assert_eq!(
        shown,
        [(
            "request[*].header:authorization:value",
            verdict("<sig:ok>"),
            verdict("<sig:bad>")
        )]
    );
    assert!(named(&vanilla(good)?, &vanilla(good)?)?.is_empty());
    Ok(())
}

#[test]
fn a_backstop_only_difference_shows_beside_a_named_one() -> Result<(), TaskError> {
    let agent = |name: &str, spacing: &str| {
        sending(
            &[
                (name, "T.A"),
                ("User-Agent", &format!("aws-cli/2{spacing}lang/rust#1")),
            ],
            "/",
            "",
        )
    };
    let form = [
        ("Host", "h"),
        ("Content-Type", "application/x-www-form-urlencoded"),
    ];
    let absolute = sending(&[("Host", "h")], "http://h/p", "");
    let mut moved = sending(&[("Host", "h")], "/p", "");
    moved["requests"][0]["method"] = json!("PUT");
    both_named(&[
        (
            agent("X-Amz-Target", " "),
            agent("x-amz-target", "  "),
            [
                "request[*].header:x-amz-target:case",
                "request[*].header:user-agent:spacing",
            ],
        ),
        (
            sending(&[("Host", "h")], "/p?a=1&&b=2&c=1", ""),
            sending(&[("Host", "h")], "/p?a=1&b=2&&c=2", ""),
            ["request[*].line:query:c", "request[*].line:query-format"],
        ),
        (
            sending(&form, "/", "A=1&&B=2&C=1"),
            sending(&form, "/", "A=1&B=2&&C=2"),
            ["request[*].body:C", "request[*].body-format"],
        ),
        (
            absolute,
            moved,
            ["request[*].line:method", "request[*].line:target-form"],
        ),
    ])
}

#[test]
fn a_known_text_difference_covers_only_its_own_lines() -> Result<(), TaskError> {
    let error = |tail: &str| {
        printing(&format!(
            "An error occurred (ThrottlingException) when calling the ListTables operation {tail}\n"
        ))
    };
    let benign = (
        run_of("r", error("(reached max retries: 4): Rate exceeded"))?,
        run_of("r", error("(reached max retries: 4): Rate exceeded."))?,
    );
    let retries = (
        run_of("r", error("(reached max retries: 4): Rate exceeded"))?,
        run_of("r", error("(reached max retries: 1): Rate exceeded"))?,
    );
    let first = build(&run(Route::Native), &silent(&["r"])?, &[benign], &[]);
    let proposed = first
        .proposed
        .first()
        .map(|proposal| proposal.fingerprint.clone())
        .unwrap_or_default();
    let lines = "(reached max retries: 4): Rate exceeded->An error occurred (ThrottlingException) \
                 when calling the ListTables operation (reached max retries: 4): Rate exceeded.";
    assert!(proposed.ends_with(lines), "{proposed}");
    let entries: Vec<Known> = serde_json::from_str(
        &entry(&proposed, "determinism").replace("\"cases\": \"*\"", "\"cases\": [\"r\"]"),
    )
    .map_err(|error| deps::error(error.to_string()))?;
    let benign = (
        run_of("r", error("(reached max retries: 4): Rate exceeded"))?,
        run_of("r", error("(reached max retries: 4): Rate exceeded."))?,
    );
    let covered = build(&run(Route::Native), &silent(&["r"])?, &[benign], &entries);
    assert_eq!((covered.known, covered.mismatched), (1, 0));
    let masked = first
        .groups
        .first()
        .map(|group| group.fingerprint.clone())
        .unwrap_or_default();
    let by_group: Vec<Known> = serde_json::from_str(&entry(&masked, "determinism"))
        .map_err(|error| deps::error(error.to_string()))?;
    let report = build(
        &run(Route::Native),
        &silent(&["r"])?,
        &[retries],
        &[entries, by_group].concat(),
    );
    assert_eq!((report.known, report.mismatched), (0, 1));
    Ok(())
}

#[test]
fn admission_changes_and_opaque_bodies_for_every_row_are_never_known() -> Result<(), TaskError> {
    assert!(known(&entry("admission-changed:a", "baseline")).is_err());
    assert!(known(&entry("request[*].body:bytes", "baseline")).is_err());
    let listed = entry("request[*].body:bytes", "baseline").replace("\"*\"", "[\"r\"]");
    assert!(known(&listed).is_ok());
    let stderr = "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable \
                  (unsupported_option); Python fallback disabled\n";
    let declined = json!({"exit": 255, "stderr": stderr});
    let pairs = [(
        run_of("r", declined.clone())?,
        run_of("r", json!({"stdout": "{}"}))?,
    )];
    let changed: Vec<Known> = serde_json::from_str(&entry("admission-changed:a", "determinism"))
        .map_err(|error| deps::error(error.to_string()))?;
    let report = build(&run(Route::Native), &silent(&["r"])?, &pairs, &changed);
    assert_eq!((report.not_native, report.mismatched), (0, 1));
    let oracle = build(&run(Route::Python), &silent(&["r"])?, &pairs, &[]);
    assert_eq!(
        (
            oracle.not_native,
            oracle.mismatched,
            oracle.verdict().is_ok()
        ),
        (1, 0, true)
    );
    Ok(())
}

// B3.2 round 2: each test fails on dd9030286.

#[test]
fn a_bad_signature_on_both_sides_is_a_mismatch_no_entry_covers() -> Result<(), TaskError> {
    let tampered = "6fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31";
    let pairs = [(vanilla(tampered)?, vanilla(tampered)?)];
    let report = build(&run(Route::Native), &rows(&["r"])?, &pairs, &[]);
    assert_eq!((report.identical, report.mismatched), (0, 1));
    assert_eq!(fingerprints(&report), ["sig:bad:a", "sig:bad:b"]);
    assert!(known(&entry("sig:bad:a", "determinism")).is_err());
    // A bad signature fails a row a native side declined, too.
    let stderr = "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable \
                  (unsupported_option); Python fallback disabled\n";
    let declined = run_of("r", json!({"exit": 255, "stderr": stderr}))?;
    let pairs = [(declined, vanilla(tampered)?)];
    let report = build(&run(Route::Python), &rows(&["r"])?, &pairs, &[]);
    assert_eq!((report.not_native, report.mismatched), (0, 1));
    assert_eq!(fingerprints(&report), ["sig:bad:b"]);
    Ok(())
}

#[test]
fn no_entry_covers_the_differences_past_the_named_ones() -> Result<(), TaskError> {
    let lines = |last: &str| {
        let mut text: String = (1..=20).map(|n| format!("line {n} a\n")).collect();
        text.push_str(last);
        text
    };
    let other: String = (1..=20).map(|n| format!("line {n} b\n")).collect();
    let pair = |last: &str| -> Result<(Capture, Capture), TaskError> {
        Ok((
            run_of("r", printing(&lines(last)))?,
            run_of("r", printing(&format!("{other}reached max retries: 1\n")))?,
        ))
    };
    let first = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[pair("reached max retries: 1\n")?],
        &[],
    );
    assert!(fingerprints(&first).contains(&"stdout:text:..."));
    let proposed: Vec<String> = first
        .proposed
        .iter()
        .map(|proposal| proposal.fingerprint.clone())
        .collect();
    let accepted: Vec<String> = proposed
        .iter()
        .map(|fingerprint| {
            let fields = r#""cases": ["r"], "mode": "determinism", "reason": "r""#;
            format!(r#"{{"fingerprint": {fingerprint:?}, {fields}}}"#)
        })
        .collect();
    let text = format!("[{}]", accepted.join(","));
    assert!(
        known(&text).is_err(),
        "an entry for an overflow was accepted"
    );
    let entries: Vec<Known> =
        serde_json::from_str(&text).map_err(|error| deps::error(error.to_string()))?;
    let report = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[pair("reached max retries: 4\n")?],
        &entries,
    );
    assert_eq!((report.known, report.mismatched), (0, 1));
    assert!(known(&entry("request[*].body:...", "determinism")).is_err());
    Ok(())
}

// B3.3 (G-042b). On 17a368a34 a row that sent nothing passed; these name the
// field it lacked, so they do not build there.

fn sends_one() -> Value {
    json!({"requests": [request("/", &[("X-Amz-Target", "T.A")], "{}")]})
}

#[test]
fn a_row_that_sends_no_request_undeclared_fails() -> Result<(), TaskError> {
    // Evasion: an argument the CLI takes for its own (a global `--version`)
    // ends the run before any request, and both sides agree.
    let quiet = || run_of("r", json!({"stdout": "aws-cli/2\n"}));
    let report = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[(quiet()?, quiet()?)],
        &[],
    );
    assert_eq!((report.identical, report.mismatched), (0, 1));
    assert_eq!(
        fingerprints(&report),
        ["requests:declared:none:a", "requests:declared:none:b"]
    );
    assert!(report.verdict().is_err());
    // A usage error ends the run as early, with its own exit status.
    let usage = || run_of("r", json!({"exit": 252, "stderr": "usage: aws\n"}));
    let report = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[(usage()?, usage()?)],
        &[],
    );
    assert_eq!(report.mismatched, 1);
    // No entry covers it, in any mode.
    for mode in ["determinism", "baseline", "oracle"] {
        for side in ["a", "b"] {
            for declared in ["none", "0"] {
                let text = entry(&format!("requests:declared:{declared}:{side}"), mode);
                assert!(known(&text).is_err(), "{text}");
            }
        }
    }
    // Declared, it is identical.
    let report = build(
        &run(Route::Native),
        &silent(&["r"])?,
        &[(quiet()?, quiet()?)],
        &[],
    );
    assert_eq!((report.identical, report.mismatched), (1, 0));
    Ok(())
}

#[test]
fn a_row_that_declares_none_must_send_none_on_each_side() -> Result<(), TaskError> {
    // Evasion: declaring 0 on a row to pass it while one side sends.
    let quiet = run_of("r", json!({}))?;
    let sending = run_of("r", sends_one())?;
    let report = build(
        &run(Route::Native),
        &silent(&["r"])?,
        &[(quiet, sending.clone())],
        &[],
    );
    let names = fingerprints(&report);
    assert!(names.contains(&"requests:declared:0:b"), "{names:?}");
    assert!(!names.contains(&"requests:declared:0:a"), "{names:?}");
    let both = build(
        &run(Route::Native),
        &silent(&["r"])?,
        &[(sending.clone(), sending)],
        &[],
    );
    assert_eq!(
        fingerprints(&both),
        ["requests:declared:0:a", "requests:declared:0:b"]
    );
    Ok(())
}

#[test]
fn a_declined_hung_or_unseen_run_is_left_to_its_own_fingerprint() -> Result<(), TaskError> {
    let stderr = "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable \
                  (unsupported_option); Python fallback disabled\n";
    let declined = run_of("r", json!({"exit": 255, "stderr": stderr}))?;
    // The oracle: the native side declined, the Python side sent.
    let oracle = build(
        &run(Route::Python),
        &rows(&["r"])?,
        &[(declined.clone(), run_of("r", sends_one())?)],
        &[],
    );
    assert_eq!((oracle.not_native, oracle.mismatched), (1, 0));
    // Both declined alike: not native, not a missing request.
    let alike = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[(declined.clone(), declined)],
        &[],
    );
    assert_eq!((alike.not_native, alike.mismatched), (1, 0));
    // But a declined side does not excuse the other's silence.
    let silent_python = build(
        &run(Route::Python),
        &rows(&["r"])?,
        &[(
            run_of("r", json!({"exit": 255, "stderr": stderr}))?,
            run_of("r", json!({}))?,
        )],
        &[],
    );
    assert_eq!(fingerprints(&silent_python), ["requests:declared:none:b"]);
    let hung = || run_of("r", json!({"exit": null, "timed_out": true}));
    let report = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[(hung()?, hung()?)],
        &[],
    );
    assert_eq!(fingerprints(&report), ["timeout:both"]);
    Ok(())
}

#[test]
fn a_run_that_compares_no_row_says_so() -> Result<(), TaskError> {
    // rev-b33 N4: a run whose every row a native side declined passes, and
    // its summary says it compared nothing.
    let stderr = "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable \
                  (unsupported_option); Python fallback disabled\n";
    let declined = || run_of("r", json!({"exit": 255, "stderr": stderr}));
    let none = build(
        &run(Route::Native),
        &rows(&["r"])?,
        &[(declined()?, declined()?)],
        &[],
    );
    assert!(none.verdict().is_ok());
    assert!(none.summary().contains("warning: no row was compared"));
    let compared = report(Route::Native)?;
    assert!(!compared.summary().contains("warning"));
    Ok(())
}
