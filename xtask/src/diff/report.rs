//! The verdict: each row's differences, grouped by fingerprint, against the
//! known differences.
//!
//! `tests/diff/known-differences.json` is a list of entries, each a
//! `fingerprint`, the `cases` (row ids, or `"*"` for every row) it holds
//! for, an optional `mode` (`oracle`, `baseline` or `determinism`; any mode
//! without one) and the `reason`. A row is identical, known (every
//! difference has an entry) or mismatched. The run passes when no row is
//! mismatched. The report proposes an entry for each unknown group, with
//! its rows and an empty reason, and lists the entries no row used.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::capture::Capture;
use super::compare::{self, Diff, Outcome};
use super::corpus::Row;
use super::{Run, Side, normalize};
use crate::{TaskError, deps};

/// The version of `report.json`.
const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Known {
    pub fingerprint: String,
    pub cases: Cases,
    #[serde(default)]
    pub mode: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Cases {
    Every(Every),
    Listed(Vec<String>),
}

/// `"*"`: every row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Every {
    #[serde(rename = "*")]
    All,
}

impl Known {
    fn covers(&self, mode: &str, id: &str, fingerprint: &str) -> bool {
        let row = match &self.cases {
            Cases::Every(Every::All) => true,
            Cases::Listed(ids) => ids.iter().any(|listed| listed == id),
        };
        row && self.fingerprint == fingerprint && self.mode.as_deref().is_none_or(|m| m == mode)
    }
}

/// The known differences in `text`.
pub fn known(text: &str) -> Result<Vec<Known>, TaskError> {
    let known: Vec<Known> = serde_json::from_str(text)
        .map_err(|error| deps::error(format!("known differences: {error}")))?;
    if let Some(entry) = known.iter().find(|entry| entry.reason.trim().is_empty()) {
        return Err(deps::error(format!(
            "the known difference {} gives no reason",
            entry.fingerprint
        )));
    }
    let modes = ["oracle", "baseline", "determinism"];
    let unknown_mode = |entry: &&Known| entry.mode.as_deref().is_some_and(|m| !modes.contains(&m));
    if let Some(entry) = known.iter().find(unknown_mode) {
        return Err(deps::error(format!(
            "the known difference {} has mode {:?}, not one of {}",
            entry.fingerprint,
            entry.mode.as_deref().unwrap_or_default(),
            modes.join(", ")
        )));
    }
    Ok(known)
}

#[derive(Debug, Serialize)]
pub struct Report {
    format: u32,
    mode: &'static str,
    a: Described,
    b: Described,
    corpus: Vec<String>,
    pub rows: usize,
    pub identical: usize,
    pub known: usize,
    pub mismatched: usize,
    pub groups: Vec<Group>,
    proposed: Vec<Proposal>,
    unused_known: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Described {
    side: String,
    binary: String,
}

impl Described {
    fn new(side: &Side) -> Self {
        Self {
            side: side.describe(),
            binary: side.binary.display().to_string(),
        }
    }
}

/// The rows one fingerprint names.
#[derive(Debug, Serialize)]
pub struct Group {
    pub fingerprint: String,
    /// Whether a known difference covers each of its rows.
    pub known: bool,
    instances: Vec<Instance>,
}

#[derive(Debug, Serialize)]
struct Instance {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<usize>,
    a: String,
    b: String,
    known: bool,
}

#[derive(Debug, Serialize)]
struct Proposal {
    fingerprint: String,
    cases: Vec<String>,
    mode: &'static str,
    reason: String,
}

/// The differences of one row's runs on A and B.
pub fn row_diffs(row: &Row, a: &Capture, b: &Capture) -> Vec<Diff> {
    compare::diffs(&outcome(row, a), &outcome(row, b))
}

fn outcome<'a>(row: &Row, capture: &'a Capture) -> Outcome<'a> {
    Outcome {
        exit: capture.exit,
        timed_out: capture.timed_out,
        stdout: &capture.stdout,
        stderr: &capture.stderr,
        requests: normalize::requests(&capture.requests, row.inputs()),
    }
}

pub fn build(run: &Run, rows: &[Row], pairs: &[(Capture, Capture)], known: &[Known]) -> Report {
    let mode = run.mode();
    let mut report = Report {
        format: FORMAT,
        mode,
        a: Described::new(&run.a),
        b: Described::new(&run.b),
        corpus: run
            .corpus
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        rows: rows.len(),
        identical: 0,
        known: 0,
        mismatched: 0,
        groups: Vec::new(),
        proposed: Vec::new(),
        unused_known: Vec::new(),
    };
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut used = BTreeSet::new();
    for (row, (a, b)) in rows.iter().zip(pairs) {
        let diffs = row_diffs(row, a, b);
        let mut covered = 0;
        for diff in &diffs {
            let entry = known
                .iter()
                .position(|k| k.covers(mode, &row.id, &diff.fingerprint));
            used.extend(entry);
            covered += usize::from(entry.is_some());
            add(&mut groups, &row.id, diff, entry.is_some());
        }
        match (diffs.len(), covered == diffs.len()) {
            (0, _) => report.identical += 1,
            (_, true) => report.known += 1,
            (_, false) => report.mismatched += 1,
        }
    }
    report.groups = ordered(groups);
    report.proposed = proposals(&report.groups, mode);
    report.unused_known = (0..known.len())
        .filter(|index| !used.contains(index))
        .map(|index| known[index].fingerprint.clone())
        .collect();
    report
}

fn add(groups: &mut BTreeMap<String, Group>, id: &str, diff: &Diff, known: bool) {
    let group = groups
        .entry(diff.fingerprint.clone())
        .or_insert_with(|| Group {
            fingerprint: diff.fingerprint.clone(),
            known: true,
            instances: Vec::new(),
        });
    group.known &= known;
    group.instances.push(Instance {
        id: id.to_owned(),
        request: diff.request,
        a: diff.a.clone(),
        b: diff.b.clone(),
        known,
    });
}

/// Unknown groups first, then by rows, most first, then by fingerprint.
fn ordered(groups: BTreeMap<String, Group>) -> Vec<Group> {
    let mut groups: Vec<Group> = groups.into_values().collect();
    groups.sort_by(|x, y| {
        (
            x.known,
            std::cmp::Reverse(x.instances.len()),
            &x.fingerprint,
        )
            .cmp(&(
                y.known,
                std::cmp::Reverse(y.instances.len()),
                &y.fingerprint,
            ))
    });
    groups
}

fn proposals(groups: &[Group], mode: &'static str) -> Vec<Proposal> {
    let unknown = groups.iter().filter(|group| !group.known);
    unknown
        .map(|group| Proposal {
            fingerprint: group.fingerprint.clone(),
            cases: group
                .instances
                .iter()
                .filter(|instance| !instance.known)
                .map(|instance| instance.id.clone())
                .collect(),
            mode,
            reason: String::new(),
        })
        .collect()
}

impl Report {
    pub fn json(&self) -> Result<String, TaskError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| deps::error(format!("diff report: {error}")))
    }

    /// The groups' fingerprints, in report order.
    pub fn fingerprints(&self) -> Vec<&str> {
        self.groups
            .iter()
            .map(|group| group.fingerprint.as_str())
            .collect()
    }

    /// The counts, then a line per group with its first instance.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "diff: A {} ({}), B {} ({}); mode {}\n\
             diff: {} rows: {} identical, {} known, {} mismatched\n",
            self.a.side,
            self.a.binary,
            self.b.side,
            self.b.binary,
            self.mode,
            self.rows,
            self.identical,
            self.known,
            self.mismatched
        );
        for group in &self.groups {
            let status = if group.known { "known" } else { "NEW" };
            let first = group.instances.first();
            let example = first.map_or_else(String::new, |instance| {
                format!("{}: {:?} vs {:?}", instance.id, instance.a, instance.b)
            });
            let rows = group.instances.len();
            let _ = writeln!(
                text,
                "  {rows:>4} {status:<5} {}  {example}",
                group.fingerprint
            );
        }
        if !self.unused_known.is_empty() {
            let _ = writeln!(
                text,
                "diff: unused known differences: {}",
                self.unused_known.join(", ")
            );
        }
        text
    }

    pub fn verdict(&self) -> Result<(), TaskError> {
        if self.mismatched == 0 {
            return Ok(());
        }
        Err(deps::error(format!(
            "{} of {} rows differ outside the known differences",
            self.mismatched, self.rows
        )))
    }
}

#[cfg(test)]
mod tests;
