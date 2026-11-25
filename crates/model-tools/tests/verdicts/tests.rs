use model_tools::verdicts::{HEADER, Row, root_rows, roots};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, model_tools::Error>;

/// One thread's rows, each root's by its index, or why it found none.
type Found = std::result::Result<Vec<(usize, Vec<Row>)>, String>;

/// Differences listed before the rest are only counted.
const SHOWN: usize = 60;

/// The most threads the rows are found on, each with its own source and
/// its own copy of the payload.
const THREADS: usize = 8;

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/verdicts/verdicts.tsv")
}

/// An empty home directory, so no `~/.aws/models` joins the shipped ones.
fn empty_home() -> Result<std::ffi::OsString> {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("verdicts-home");
    std::fs::create_dir_all(&home)?;
    Ok(home.into_os_string())
}

/// The rows of a TSV text by `command\toperation`, the header left out.
fn keyed(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .skip(1)
        .map(|line| {
            let mut fields = line.splitn(3, '\t');
            let key = [fields.next(), fields.next()].map(Option::unwrap_or_default);
            let verdict = fields.next().unwrap_or_default().replace('\t', " | ");
            (key.join(" "), verdict)
        })
        .collect()
}

/// Each row the two texts disagree on, one line each.
fn differences(expected: &str, found: &str) -> Vec<String> {
    let (expected, found) = (keyed(expected), keyed(found));
    let mut differences = Vec::new();
    for (key, verdict) in &expected {
        match found.get(key) {
            None => differences.push(format!("- {key}: {verdict}")),
            Some(now) if now != verdict => {
                differences.push(format!("~ {key}: {verdict}  ->  {now}"));
            }
            Some(_) => {}
        }
    }
    for (key, verdict) in &found {
        if !expected.contains_key(key) {
            differences.push(format!("+ {key}: {verdict}"));
        }
    }
    differences
}

/// `verdicts::rows` over the shipped models, on up to [`THREADS`]
/// threads: thread `t` finds the rows of every root whose index is `t`
/// modulo the thread count, and the rows are joined in root order.
fn shipped_rows() -> Result<Vec<Row>> {
    let home = empty_home()?;
    let roots = model_tools::with_source(home.clone(), None, roots)?;
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get().min(THREADS));
    let found: Vec<Found> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|thread| {
                let (home, roots) = (home.clone(), &roots);
                scope.spawn(move || {
                    model_tools::with_source(home, None, |source| {
                        let mine = roots.iter().enumerate().skip(thread).step_by(threads);
                        mine.map(|(index, root)| (index, root_rows(source, root)))
                            .collect()
                    })
                    .map_err(|error| error.to_string())
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| Err("a thread panicked".into()))
            })
            .collect()
    });
    let mut by_root = Vec::new();
    for rows in found {
        by_root.extend(rows?);
    }
    by_root.sort_by_key(|(index, _)| *index);
    Ok(by_root.into_iter().flat_map(|(_, rows)| rows).collect())
}

#[test]
fn every_shipped_operation_keeps_its_verdict() -> Result<()> {
    let mut found = format!("{HEADER}\n");
    for row in shipped_rows()? {
        found.push_str(&row.line());
        found.push('\n');
    }
    let expected = std::fs::read_to_string(golden())?;
    if found == expected {
        return Ok(());
    }
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("verdicts.tsv");
    std::fs::write(&scratch, &found)?;
    let differences = differences(&expected, &found);
    let mut report = format!(
        "{} rows differ from {} (- gone, + new, ~ changed; {}):\n",
        differences.len(),
        golden().display(),
        HEADER.replace('\t', " | "),
    );
    for difference in differences.iter().take(SHOWN) {
        writeln!(report, "  {difference}")?;
    }
    if differences.len() > SHOWN {
        writeln!(report, "  ... and {} more", differences.len() - SHOWN)?;
    }
    if differences.is_empty() {
        report.push_str("  the rows agree; their order or the header differs\n");
    }
    writeln!(
        report,
        "the rows found are in {}; a verdict change the user signs off is accepted by copying \
         that file over the golden one",
        scratch.display()
    )?;
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    Err(format!("{} verdicts differ from the golden file", differences.len()).into())
}
