use super::*;
use crate::diff::corpus;

const CAPTURE: &str = concat!(
    r#"{"id": "r", "side": "a", "exit": 0, "timed_out": false, "stdout": "\u00ff{}", "#,
    r#""stderr": "", "requests": [{"method": "GET", "target": "/", "version": "1.1", "#,
    r#""headers": [["Host", "h\u00e9"]], "body": "\u0000", "complete": true}]}"#
);

fn capture(id: &str, side: &str) -> Result<Capture, TaskError> {
    let text =
        CAPTURE
            .replacen("\"r\"", &format!("{id:?}"), 1)
            .replacen("\"a\"", &format!("{side:?}"), 1);
    parse(&text)?.pop().ok_or_else(|| deps::error("no capture"))
}

#[test]
fn byte_strings_arrive_as_latin_1_text() -> Result<(), TaskError> {
    let capture = capture("r", "a")?;
    assert_eq!(capture.stdout, b"\xff{}");
    let request = capture.requests.first().cloned();
    assert_eq!(request.as_ref().map(|r| r.body.clone()), Some(vec![0]));
    assert_eq!(
        request.map(|r| r.headers),
        Some(vec![("Host".to_owned(), "h\u{e9}".to_owned())])
    );
    let wide = CAPTURE.replace("\\u00ff{}", "\\u0100");
    let error = parse(&wide)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(error.starts_with("diff capture: "), "{error}");
    Ok(())
}

#[test]
fn each_row_pairs_its_run_on_a_with_its_run_on_b() -> Result<(), TaskError> {
    let rows = corpus::parse(
        "rows",
        concat!(
            "{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[]}\n",
            "{\"id\":\"s\",\"argv\":[\"x\"],\"responses\":[]}",
        ),
    )?;
    let captures = vec![
        capture("r", "a")?,
        capture("r", "b")?,
        capture("s", "a")?,
        capture("s", "b")?,
    ];
    let pairs = pair(&rows, captures)?;
    let ids: Vec<_> = pairs
        .iter()
        .map(|(a, b)| (a.id.as_str(), b.side.as_str()))
        .collect();
    assert_eq!(ids, [("r", "b"), ("s", "b")]);
    let missing = pair(
        &rows,
        vec![capture("r", "a")?, capture("r", "b")?, capture("s", "a")?],
    );
    assert_eq!(
        missing.err().map(|error| error.to_string()).as_deref(),
        Some("the diff capture does not hold each row once per side")
    );
    let unknown = pair(&rows, vec![capture("r", "c")?]);
    assert_eq!(
        unknown.err().map(|error| error.to_string()).as_deref(),
        Some("diff capture of unknown side c")
    );
    Ok(())
}
