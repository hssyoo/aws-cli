use model_tools::verdicts::{HEADER, Row, UNLOADED, root_rows, rows};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, model_tools::Error>;

/// Differences listed before the rest are only counted.
const SHOWN: usize = 60;

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/verdicts/verdicts.tsv")
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A scratch directory of this test binary's, created empty.
fn scratch(name: &str) -> Result<PathBuf> {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    if directory.exists() {
        std::fs::remove_dir_all(&directory)?;
    }
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

/// The rows as the TSV holds them: the header, then one line per row.
fn text(rows: &[Row]) -> String {
    let mut text = format!("{HEADER}\n");
    for row in rows {
        text.push_str(&row.line());
        text.push('\n');
    }
    text
}

/// The verdict fields of a TSV text by `command operation`, every line a
/// key holds kept, the header left out.
fn keyed(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut keyed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in text.lines().skip(1) {
        let mut fields = line.splitn(3, '\t');
        let key = [fields.next(), fields.next()].map(Option::unwrap_or_default);
        let verdict = fields.next().unwrap_or_default().replace('\t', " | ");
        keyed.entry(key.join(" ")).or_default().push(verdict);
    }
    keyed
}

/// Each row the two texts disagree on, one line each: `-` gone, `+` new,
/// `~` changed, `=` a key held by more than one row.
fn differences(expected: &str, found: &str) -> Vec<String> {
    let (expected, found) = (keyed(expected), keyed(found));
    let mut differences = Vec::new();
    for (side, rows) in [("golden", &expected), ("found", &found)] {
        for (key, verdicts) in rows.iter().filter(|(_, verdicts)| verdicts.len() > 1) {
            differences.push(format!(
                "= {key}: {} rows in the {side} text",
                verdicts.len()
            ));
        }
    }
    for (key, verdict) in &expected {
        match found.get(key) {
            None => differences.push(format!("- {key}: {}", verdict.join(" ; "))),
            Some(now) if now != verdict => differences.push(format!(
                "~ {key}: {}  ->  {}",
                verdict.join(" ; "),
                now.join(" ; ")
            )),
            Some(_) => {}
        }
    }
    for (key, verdict) in &found {
        if !expected.contains_key(key) {
            differences.push(format!("+ {key}: {}", verdict.join(" ; ")));
        }
    }
    differences
}

/// Each row whose refusal and standing disagree: a static refusal is the
/// first thing `admit::operation` checks, so its reason is the standing's.
fn inconsistent(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.refusal != "-" && row.standing != format!("refused:{}", row.refusal))
        .map(|row| {
            format!(
                "! {} {}: refusal {} but standing {}",
                row.command, row.operation, row.refusal, row.standing
            )
        })
        .collect()
}

/// What the test reports when the rows found are not the golden text, or
/// `None` when they are.
fn report(expected: &str, found: &str, rows: &[Row]) -> Option<String> {
    let mut lines = inconsistent(rows);
    if found != expected {
        let differences = differences(expected, found);
        if differences.is_empty() {
            lines.push("the rows agree; their order, the header or the line ends differ".into());
        }
        lines.extend(differences);
    }
    if lines.is_empty() {
        return None;
    }
    let mut report = format!(
        "{} findings against {} (- gone, + new, ~ changed, = duplicated, ! inconsistent; {}):\n",
        lines.len(),
        golden().display(),
        HEADER.replace('\t', " | "),
    );
    for line in lines.iter().take(SHOWN) {
        let _ = writeln!(report, "  {line}");
    }
    if lines.len() > SHOWN {
        let _ = writeln!(report, "  ... and {} more", lines.len() - SHOWN);
    }
    Some(report)
}

/// The shipped models (the payload's, no `AWS_DATA_PATH`, an empty home)
/// through `verdicts::rows`, the oracle's entry point, against the golden
/// text.
#[test]
fn every_shipped_operation_keeps_its_verdict() -> Result<()> {
    let home = scratch("verdicts-home")?.into_os_string();
    let rows = model_tools::with_source(home, None, rows)?;
    let found = text(&rows);
    let expected = std::fs::read_to_string(golden())?;
    let Some(mut report) = report(&expected, &found, &rows) else {
        return Ok(());
    };
    let rows_found = Path::new(env!("CARGO_TARGET_TMPDIR")).join("verdicts.tsv");
    std::fs::write(&rows_found, &found)?;
    let _ = writeln!(
        report,
        "the rows found are in {}; a verdict change the user signs off is accepted by copying \
         that file over the golden one",
        rows_found.display()
    );
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    Err("the verdicts differ from the golden file".into())
}

/// A row of the given standing, with nothing refused.
fn row(command: &str, operation: &str, standing: &str) -> Row {
    Row {
        command: command.into(),
        operation: operation.into(),
        refusal: "-".into(),
        refused: "-".into(),
        standing: standing.into(),
        paginator: "-".into(),
        api_version: "2020-01-01".into(),
    }
}

/// Evasion 1: a duplicated row, whose key the keyed comparison holds once,
/// is named as duplicated rather than passing as agreeing rows.
#[test]
fn a_duplicated_row_is_named() {
    let rows = [row("a", "x", "no_arguments"), row("a", "y", "no_arguments")];
    let found = [rows[0].clone(), rows[1].clone(), rows[1].clone()];
    let report = report(&text(&rows), &text(&found), &found).unwrap_or_default();
    assert!(
        report.contains("= a y: 2 rows in the found text"),
        "{report}"
    );
}

/// Evasion 2: a static refusal whose reason the standing does not carry
/// (a check inserted before `static_refusal`) is named even when the
/// golden text holds the same drift.
#[test]
fn a_refusal_the_standing_disagrees_with_is_named() {
    let mut drifted = row("a", "x", "refused:unported_customization");
    drifted.refusal = "unsupported_trait".into();
    drifted.refused = "streaming".into();
    let rows = [drifted];
    let report = report(&text(&rows), &text(&rows), &rows).unwrap_or_default();
    assert!(
        report.contains(
            "! a x: refusal unsupported_trait but standing refused:unported_customization"
        ),
        "{report}"
    );
}

/// Evasion 3: a field holding a tab or a newline stays one cell, so it
/// cannot shift a verdict into another column or forge a row.
#[test]
fn a_field_with_a_tab_or_newline_stays_one_cell() {
    let mut forged = row("a", "x", "no_arguments");
    forged.refused = "one\ttwo\nb\ty".into();
    let line = forged.line();
    assert_eq!(
        line.split('\t').count(),
        HEADER.split('\t').count(),
        "{line}"
    );
    assert!(!line.contains('\n'));
    assert!(line.contains("one\\ttwo\\nb\\ty"), "{line}");
}

/// Evasion 4: an empty field is not rendered as `-`, so a name that goes
/// missing does not read as no refusal.
#[test]
fn an_empty_field_is_not_none() {
    let mut emptied = row("a", "x", "refused:unsupported_trait");
    emptied.refusal = "unsupported_trait".into();
    emptied.refused = String::new();
    let mut none = emptied.clone();
    none.refused = "-".into();
    assert_ne!(emptied.line(), none.line());
    assert!(emptied.line().contains("\t\"\"\t"), "{}", emptied.line());
}

/// Evasion 5: rows reordered, a header changed or line ends changed are
/// reported, though every key keeps its verdict.
#[test]
fn order_header_and_line_ends_are_compared() {
    let rows = [
        row("a", "x", "no_arguments"),
        row("a", "y", "scalar_arguments"),
    ];
    let golden = text(&rows);
    let reordered = [rows[1].clone(), rows[0].clone()];
    let changes = [
        text(&reordered),
        golden.replacen("api_version", "version", 1),
        golden.replace('\n', "\r\n"),
        golden.trim_end().to_owned(),
    ];
    for found in changes {
        let report = report(&golden, &found, &rows).unwrap_or_default();
        assert!(report.contains("the rows agree"), "{found:?}: {report}");
    }
}

/// Evasion 6: a root that stops loading keeps a row that says so, rather
/// than vanishing with its reason lost.
#[test]
fn a_root_that_does_not_load_has_an_unloaded_row() -> Result<()> {
    let home = scratch("verdicts-home-unloaded")?.into_os_string();
    let found = model_tools::with_source(home, None, |source| root_rows(source, "no-such-root"))?;
    assert_eq!(
        found,
        [Row {
            paginator: "-".into(),
            api_version: "-".into(),
            ..row("no-such-root", "-", UNLOADED)
        }]
    );
    Ok(())
}

/// The fake `zjson` model, its paginators file edited by `edit`, as the
/// only model of a data path; the rows of its root.
fn zjson_rows(name: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Result<Vec<Row>> {
    let models = scratch(name)?;
    let original = repository().join("tests/models/zjson/2020-01-01");
    let copy = models.join("zjson/2020-01-01");
    std::fs::create_dir_all(&copy)?;
    for file in ["service-2.json", "endpoint-rule-set-1.json"] {
        std::fs::copy(original.join(file), copy.join(file))?;
    }
    let mut pages: serde_json::Value =
        serde_json::from_slice(&std::fs::read(original.join("paginators-1.json"))?)?;
    edit(&mut pages);
    std::fs::write(copy.join("paginators-1.json"), serde_json::to_vec(&pages)?)?;
    let home = scratch(&format!("{name}-home"))?.into_os_string();
    model_tools::with_source(home, Some(models.into_os_string()), |source| {
        root_rows(source, "zjson")
    })
}

/// An edit of a paginators file.
type Edit = fn(&mut serde_json::Value);

fn paginator_of<'r>(rows: &'r [Row], operation: &str) -> &'r str {
    rows.iter()
        .find(|row| row.operation == operation)
        .map_or("", |row| &row.paginator)
}

/// Evasion 7: a paginator the loader reads differently (a limit key
/// dropped, an output token swapped, a result key lost) changes the
/// paginator column, though the standing stays the same.
#[test]
fn a_paginator_config_change_shows_in_its_row() -> Result<()> {
    let original = zjson_rows("verdicts-zjson", |_| {})?;
    assert_eq!(
        paginator_of(&original, "list-keys"),
        "native in=Marker out=NextMarker more=Truncated result=Keys keep=- limit=Limit"
    );
    let edits: [(&str, Edit); 3] = [
        ("limit_key", |pages| {
            let _ = pages["pagination"]["ListKeys"]
                .as_object_mut()
                .map(|entry| entry.remove("limit_key"));
        }),
        ("output_token", |pages| {
            pages["pagination"]["ListKeys"]["output_token"] = "Marker".into();
        }),
        ("result_key", |pages| {
            pages["pagination"]["ListKeys"]["result_key"] = serde_json::json!([]);
        }),
    ];
    for (key, edit) in edits {
        let edited = zjson_rows(&format!("verdicts-zjson-{key}"), edit)?;
        let (before, after) = (
            paginator_of(&original, "list-keys"),
            paginator_of(&edited, "list-keys"),
        );
        assert_ne!(before, after, "{key}");
        assert!(
            after.starts_with("native ") || after.starts_with("refused:"),
            "{key}: {after}"
        );
    }
    Ok(())
}

/// Evasion 8: the model version the search loads is in every row, so a
/// newer version with the same verdicts still changes the text.
#[test]
fn the_api_version_is_in_every_row() -> Result<()> {
    let found = zjson_rows("verdicts-zjson-version", |_| {})?;
    assert!(!found.is_empty());
    assert!(found.iter().all(|row| row.api_version == "2020-01-01"));
    Ok(())
}
