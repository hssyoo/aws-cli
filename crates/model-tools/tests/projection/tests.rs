use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, model_tools::Error>;

/// Differences listed before the rest are only counted.
const SHOWN: usize = 40;
/// The digest of a root whose service does not load.
const UNLOADED: &str = "unloaded";
/// The first line of the digests file.
const HEADER: &str = "model\tdigest\tlines";
/// The prefix of the protocol suite cases' model directories.
const SUITE: &str = "zz-suite-";

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/projection/digests.tsv")
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

/// FNV-1a, 64 bits, over the text: stable across toolchains, unlike std's
/// hashers.
fn digest(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// One model's row: its directory, the digest of its projection and the
/// projection's line count, or [`UNLOADED`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    model: String,
    digest: String,
    lines: usize,
}

impl Row {
    fn of(model: &str, projection: Option<&str>) -> Self {
        Self {
            model: model.escape_debug().to_string(),
            digest: projection.map_or_else(|| UNLOADED.to_owned(), digest),
            lines: projection.map_or(0, |text| text.lines().count()),
        }
    }

    fn line(&self) -> String {
        format!("{}\t{}\t{}", self.model, self.digest, self.lines)
    }
}

fn text(rows: &[Row]) -> String {
    let mut text = format!("{HEADER}\n");
    for row in rows {
        text.push_str(&row.line());
        text.push('\n');
    }
    text
}

/// The rows of a digests text by model, every line a model holds kept.
fn keyed(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut keyed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in text.lines().skip(1) {
        let (model, rest) = line.split_once('\t').unwrap_or((line, ""));
        keyed
            .entry(model.to_owned())
            .or_default()
            .push(rest.to_owned());
    }
    keyed
}

/// Each model the texts disagree on: `-` gone, `+` new, `~` changed, `=`
/// held by more than one row; and a header that is not [`HEADER`].
fn differences(expected: &str, found: &str) -> Vec<String> {
    let mut differences = Vec::new();
    for (side, text) in [("golden", expected), ("found", found)] {
        if text.lines().next() != Some(HEADER) {
            differences.push(format!("! the {side} text's header is not {HEADER:?}"));
        }
    }
    let (expected, found) = (keyed(expected), keyed(found));
    for (side, rows) in [("golden", &expected), ("found", &found)] {
        for (model, digests) in rows.iter().filter(|(_, digests)| digests.len() > 1) {
            differences.push(format!(
                "= {model}: {} rows in the {side} text",
                digests.len()
            ));
        }
    }
    for (model, digest) in &expected {
        match found.get(model) {
            None => differences.push(format!("- {model}: {}", digest.join(" ; "))),
            Some(now) if now != digest => differences.push(format!(
                "~ {model}: {}  ->  {}",
                digest.join(" ; "),
                now.join(" ; ")
            )),
            Some(_) => {}
        }
    }
    for (model, digest) in &found {
        if !expected.contains_key(model) {
            differences.push(format!("+ {model}: {}", digest.join(" ; ")));
        }
    }
    differences
}

/// Each protocol suite case's description, the suite's with the case as
/// its only operation (under `given.name`, else `OperationName`), by the
/// model directory it is written to.
fn suite_models() -> Result<Vec<(String, Json)>> {
    let root = repository().join("aws-cli/tests/unit/botocore/protocols");
    let mut models = Vec::new();
    for direction in ["input", "output"] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(direction))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<_, _>>()?;
        files.sort();
        for file in files {
            let suite: Json = serde_json::from_slice(&std::fs::read(&file)?)?;
            let stem = file.file_stem().unwrap_or_default().to_string_lossy();
            for (g, group) in suite.as_array().into_iter().flatten().enumerate() {
                for (c, case) in group["cases"].as_array().into_iter().flatten().enumerate() {
                    let mut model = group.clone();
                    let object = model.as_object_mut().ok_or("a suite group")?;
                    object.remove("cases");
                    let operation = case["given"]["name"].as_str().unwrap_or("OperationName");
                    object.insert("operations".into(), json!({ operation: case["given"] }));
                    models.push((format!("{SUITE}{direction}-{stem}-{g}-{c}"), model));
                }
            }
        }
    }
    Ok(models)
}

/// A data path holding the fake models and the suite's.
fn data_path(name: &str) -> Result<(PathBuf, usize)> {
    let models = scratch(name)?;
    let fakes = repository().join("tests/models");
    let mut count = 0;
    for entry in std::fs::read_dir(&fakes)? {
        let entry = entry?;
        for version in std::fs::read_dir(entry.path())? {
            let version = version?;
            let to = models.join(entry.file_name()).join(version.file_name());
            std::fs::create_dir_all(&to)?;
            std::fs::copy(
                version.path().join("service-2.json"),
                to.join("service-2.json"),
            )?;
        }
        count += 1;
    }
    for (directory, model) in suite_models()? {
        let to = models.join(directory).join("2020-01-01");
        std::fs::create_dir_all(&to)?;
        std::fs::write(to.join("service-2.json"), serde_json::to_vec(&model)?)?;
        count += 1;
    }
    Ok((models, count))
}

/// What the test reports when the rows found are not the golden text.
fn report(expected: &str, found: &str) -> Option<String> {
    if expected == found {
        return None;
    }
    let mut lines = differences(expected, found);
    if lines.is_empty() {
        lines.push("the rows agree; their order or the line ends differ".into());
    }
    let mut report = format!(
        "{} findings against {} (- gone, + new, ~ changed, = duplicated):\n",
        lines.len(),
        golden().display()
    );
    for line in lines.iter().take(SHOWN) {
        let _ = writeln!(report, "  {line}");
    }
    if lines.len() > SHOWN {
        let _ = writeln!(report, "  ... and {} more", lines.len() - SHOWN);
    }
    Some(report)
}

/// The shipped models, the fakes and the suite's, through the CLI's model
/// source, against the golden digests.
#[test]
fn every_model_keeps_its_projection() -> Result<()> {
    let (models, written) = data_path("projection-models")?;
    let home = scratch("projection-home")?.into_os_string();
    // Every root's row, by model directory, and each loaded projection.
    let (rows, projections) = model_tools::with_source(home, Some(models.into()), |source| {
        let mut rows = Vec::new();
        let mut projections = BTreeMap::new();
        let roots = source.roots().cloned().unwrap_or_default();
        for (command, model) in &roots {
            let projection = source
                .service(command)
                .map(|service| service.model.projection());
            rows.push(Row::of(model, projection.as_deref()));
            if let Some(projection) = projection {
                projections.insert(model.clone(), projection);
            }
        }
        rows.sort_by(|a, b| a.model.cmp(&b.model));
        (rows, projections)
    })?;
    assert!(rows.len() >= 436 + written, "only {} models", rows.len());
    let found = text(&rows);
    let expected = std::fs::read_to_string(golden())?;
    let Some(mut report) = report(&expected, &found) else {
        return Ok(());
    };
    let out = scratch("projection-found")?;
    std::fs::write(out.join("digests.tsv"), &found)?;
    let changed = keyed(&expected);
    for (model, projection) in &projections {
        let row = Row::of(model, Some(projection)).line();
        let digest = row.split_once('\t').map_or("", |(_, rest)| rest);
        if changed
            .get(model)
            .is_none_or(|rows| rows.iter().all(|r| r != digest))
        {
            std::fs::write(out.join(format!("{model}.txt")), projection)?;
        }
    }
    let _ = writeln!(
        report,
        "the rows found and each differing projection are in {}; a change the user signs off \
         is accepted by copying digests.tsv over the golden one",
        out.display()
    );
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    Err("the projections differ from the golden digests".into())
}

fn row(model: &str, digest: &str) -> Row {
    Row {
        model: model.into(),
        digest: digest.into(),
        lines: 1,
    }
}

/// Evasion 1: a model whose row is gone (it no longer loads as a root, or
/// the walk skipped it) is named, not passed over.
#[test]
fn a_missing_model_is_named() {
    let golden = text(&[row("a", "1"), row("b", "2")]);
    let found = text(&[row("a", "1")]);
    let report = report(&golden, &found).unwrap_or_default();
    assert!(report.contains("- b: 2\t1"), "{report}");
}

/// Evasion 2: a model the golden text does not hold is named.
#[test]
fn a_new_model_is_named() {
    let golden = text(&[row("a", "1")]);
    let found = text(&[row("a", "1"), row("c", "3")]);
    assert!(
        report(&golden, &found)
            .unwrap_or_default()
            .contains("+ c: 3\t1")
    );
}

/// Evasion 3: a duplicated row, which the keyed comparison holds once, is
/// named as duplicated.
#[test]
fn a_duplicated_row_is_named() {
    let golden = text(&[row("a", "1")]);
    let found = text(&[row("a", "1"), row("a", "1")]);
    let report = report(&golden, &found).unwrap_or_default();
    assert!(report.contains("= a: 2 rows in the found text"), "{report}");
}

/// Evasion 4: a model that stops loading changes its row to `unloaded`
/// rather than leaving it out.
#[test]
fn a_model_that_stops_loading_keeps_an_unloaded_row() {
    let loaded = Row::of("a", Some("text"));
    let unloaded = Row::of("a", None);
    assert_eq!(unloaded.digest, UNLOADED);
    let report = report(&text(&[loaded]), &text(&[unloaded])).unwrap_or_default();
    assert!(report.contains("~ a:"), "{report}");
}

/// Evasion 5: any one-byte change, a swap of two lines, or a line moved
/// between models changes the digest or the line count.
#[test]
fn the_digest_sees_every_byte_and_the_line_order() {
    let base = "shape \"A\" string \"string\"\n  member \"B\" -> \"S\"\n";
    let changes = [
        base.replace("string\"", "strinG\""),
        "  member \"B\" -> \"S\"\nshape \"A\" string \"string\"\n".to_owned(),
        base.replace('\n', "\r\n"),
        format!("{base}\n"),
        base.trim_end().to_owned(),
    ];
    for changed in changes {
        assert_ne!(digest(&changed), digest(base), "{changed:?}");
    }
}

/// Evasion 6: a model directory holding a tab or a newline stays one
/// cell, so it cannot forge a row.
#[test]
fn a_model_name_with_a_tab_or_newline_stays_one_cell() {
    let forged = Row::of("a\tb\nc", Some("x"));
    let rows = text(std::slice::from_ref(&forged));
    assert_eq!(rows.lines().count(), 2, "{rows}");
    assert_eq!(keyed(&rows).len(), 1);
}

/// Evasion 7: a golden or found text without the header, which would
/// otherwise drop its first row, is named.
#[test]
fn a_missing_header_is_named() {
    let golden = text(&[row("a", "1")]);
    let found = golden.lines().skip(1).collect::<Vec<_>>().join("\n");
    let report = report(&golden, &found).unwrap_or_default();
    assert!(report.contains("the found text's header"), "{report}");
}

/// Evasion 8: the suite models are each case's own, so a case dropped
/// from a group (or a group from a file) loses rows.
#[test]
fn every_suite_case_is_a_model() -> Result<()> {
    let models = suite_models()?;
    assert!(models.len() > 700, "{}", models.len());
    let names: std::collections::BTreeSet<&str> = models.iter().map(|(n, _)| &**n).collect();
    assert_eq!(names.len(), models.len());
    Ok(())
}
