//! The verdict: each row's differences, grouped by fingerprint, against the
//! known differences.
//!
//! `tests/diff/known-differences.json` is a list of entries, each a
//! `fingerprint`, the `cases` (row ids, or `"*"` for every row) it holds
//! for, the `mode` (`oracle`, `baseline` or `determinism`) it holds in, and
//! the `reason`. A row is identical, known (every difference has an entry)
//! or mismatched, or not native ([`admission`]). A timeout, a harness error,
//! a changed admission, a bad signature on either side ([`UNCOVERABLE`]) or
//! a stage's overflow past its named differences ([`OVERFLOW`]) is never
//! known: no entry may name one. So is a run that sends no request when its
//! row does not declare `"requests": 0`, or one that sends a request when it
//! does. A text difference is covered only by an entry naming its full
//! lines (its `key`), and a `:bytes` one only for listed rows. The run
//! passes when no row is mismatched. The report also counts each side's
//! signatures by verdict ([`sigv4`]).
//! The report proposes an entry for each unknown group, with its rows and
//! an empty reason, and lists the entries no row used.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::admission::{self, Refusals};
use super::capture::Capture;
use super::compare::{self, Diff, Outcome};
use super::corpus::Row;
use super::sigv4::{self, Verdict};
use super::{Run, Side, normalize};
use crate::{TaskError, deps};

/// The version of `report.json`.
const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Known {
    pub fingerprint: String,
    pub cases: Cases,
    pub mode: String,
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
    fn covers(&self, mode: &str, id: &str, diff: &Diff) -> bool {
        let fingerprint = diff.key.as_deref().unwrap_or(&diff.fingerprint);
        let row = match &self.cases {
            Cases::Every(Every::All) => true,
            Cases::Listed(ids) => ids.iter().any(|listed| listed == id),
        };
        row && self.fingerprint == fingerprint && self.mode == mode
    }
}

/// The fingerprints no entry may cover: the run was not seen whole, a
/// request's signature is bad, or a run broke its row's declared requests
/// ([`declared`]).
pub const UNCOVERABLE: [&str; 5] = [
    "timeout:",
    "harness:",
    "admission-changed:",
    "sig:bad:",
    "requests:declared:",
];

/// The end of a stage's fingerprint past its last named difference, which
/// stands for any number of others, so no entry may cover it either.
pub const OVERFLOW: &str = ":...";

fn uncoverable(fingerprint: &str) -> bool {
    fingerprint.ends_with(OVERFLOW)
        || UNCOVERABLE
            .iter()
            .any(|prefix| fingerprint.starts_with(prefix))
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
    if let Some(entry) = known
        .iter()
        .find(|entry| !modes.contains(&entry.mode.as_str()))
    {
        return Err(deps::error(format!(
            "the known difference {} has mode {:?}, not one of {}",
            entry.fingerprint,
            entry.mode,
            modes.join(", ")
        )));
    }
    if let Some(entry) = known.iter().find(|entry| uncoverable(&entry.fingerprint)) {
        return Err(deps::error(format!(
            "the known difference {} names a timeout, a harness error, an admission change, \
             a bad signature, a broken request declaration or an overflow, which no entry covers",
            entry.fingerprint
        )));
    }
    let every_body = |entry: &&Known| {
        entry.fingerprint.ends_with(":bytes") && matches!(entry.cases, Cases::Every(_))
    };
    if let Some(entry) = known.iter().find(every_body) {
        return Err(deps::error(format!(
            "the known difference {} must list its rows: an opaque body has no field to name",
            entry.fingerprint
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
    /// Rows not compared, as a native side declined them.
    pub not_native: usize,
    refusals: Refusals,
    signatures: BTreeMap<&'static str, Signatures>,
    pub groups: Vec<Group>,
    proposed: Vec<Proposal>,
    unused_known: Vec<String>,
}

/// A side's request signatures by verdict.
#[derive(Debug, Default, Serialize)]
struct Signatures {
    ok: usize,
    bad: usize,
    unverified: usize,
}

impl Signatures {
    /// Counts `capture`'s signatures, and gives a `sig:bad:<side>`
    /// difference at its first bad one.
    fn count(&mut self, side: &str, capture: &Capture) -> Option<Diff> {
        let mut first_bad = None;
        for (index, request) in capture.requests.iter().enumerate() {
            match sigv4::verify(request, capture.secret_key.as_deref()) {
                Verdict::Ok => self.ok += 1,
                Verdict::Bad => {
                    self.bad += 1;
                    first_bad.get_or_insert(index);
                }
                Verdict::Unverified => self.unverified += 1,
            }
        }
        first_bad.map(|index| Diff {
            fingerprint: format!("sig:bad:{side}"),
            key: None,
            request: Some(index),
            a: String::new(),
            b: String::new(),
        })
    }
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
    key: Option<String>,
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
        harness: &capture.harness,
        exit: capture.exit,
        timed_out: capture.timed_out,
        stdout: &capture.stdout,
        stderr: &capture.stderr,
        connects: &capture.connects,
        requests: normalize::requests(
            &capture.requests,
            row.inputs(),
            capture.secret_key.as_deref(),
        ),
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
        not_native: 0,
        refusals: Refusals::new(),
        signatures: BTreeMap::new(),
        groups: Vec::new(),
        proposed: Vec::new(),
        unused_known: Vec::new(),
    };
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut used = BTreeSet::new();
    for (row, (a, b)) in rows.iter().zip(pairs) {
        // A bad signature fails the row even where both sides share it, or
        // a native side declined the row.
        let bad: Vec<Diff> = [("a", a), ("b", b)]
            .into_iter()
            .filter_map(|(side, capture)| {
                let counted = report.signatures.entry(side).or_default();
                counted.count(side, capture)
            })
            .collect();
        let bad: Vec<Diff> = bad.into_iter().chain(declared(run, row, a, b)).collect();
        let mut diffs = match admission::diffs(run, row, (a, b), &mut report.refusals) {
            Some(diffs) => diffs,
            None if bad.is_empty() => {
                report.not_native += 1;
                continue;
            }
            None => Vec::new(),
        };
        diffs.extend(bad);
        let mut covered = 0;
        for diff in &diffs {
            let entry = known
                .iter()
                .position(|k| k.covers(mode, &row.id, diff))
                .filter(|_| !uncoverable(&diff.fingerprint));
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

/// A run that ends having sent no request, its row not declaring
/// `"requests": 0` (`requests:declared:none:<side>`), or that sends one
/// though it does (`requests:declared:0:<side>`). A run a native side
/// declined, that timed out or that the capture did not see whole is left to
/// its own fingerprint.
fn declared(run: &Run, row: &Row, a: &Capture, b: &Capture) -> Vec<Diff> {
    let mut found = Vec::new();
    for (name, side, capture) in [("a", &run.a, a), ("b", &run.b, b)] {
        let whole = capture.exit.is_some() && !capture.timed_out && capture.harness.is_empty();
        if !whole || admission::refusal(side, capture).is_some() {
            continue;
        }
        let sent = capture.requests.len();
        let (declared, detail) = match (row.requests, sent) {
            (None, 0) => (
                "none",
                "sent no request; a row that sends none declares \"requests\": 0".to_owned(),
            ),
            (Some(_), 1..) => ("0", format!("sent {sent} requests; the row declares 0")),
            _ => continue,
        };
        let detail = detail.as_str();
        let (x, y) = if name == "a" {
            (detail, "")
        } else {
            ("", detail)
        };
        found.push(Diff {
            fingerprint: format!("requests:declared:{declared}:{name}"),
            key: None,
            request: None,
            a: x.to_owned(),
            b: y.to_owned(),
        });
    }
    found
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
        key: diff.key.clone(),
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

/// An entry for each fingerprint, or text key, no entry covers, with the
/// rows it would cover.
fn proposals(groups: &[Group], mode: &'static str) -> Vec<Proposal> {
    let mut proposed: Vec<Proposal> = Vec::new();
    let uncovered = groups.iter().flat_map(|group| {
        let instances = group.instances.iter().filter(|instance| !instance.known);
        instances.map(|instance| {
            (
                instance.key.as_ref().unwrap_or(&group.fingerprint),
                instance,
            )
        })
    });
    for (fingerprint, instance) in uncovered {
        match proposed
            .iter_mut()
            .find(|proposal| proposal.fingerprint == *fingerprint)
        {
            Some(proposal) => proposal.cases.push(instance.id.clone()),
            None => proposed.push(Proposal {
                fingerprint: fingerprint.clone(),
                cases: vec![instance.id.clone()],
                mode,
                reason: String::new(),
            }),
        }
    }
    proposed
}

impl Report {
    pub fn json(&self) -> Result<String, TaskError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| deps::error(format!("diff report: {error}")))
    }

    /// Each differing row's fingerprints, sorted.
    pub fn by_row(&self) -> BTreeMap<&str, Vec<&str>> {
        let mut rows: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for group in &self.groups {
            for instance in &group.instances {
                rows.entry(&instance.id)
                    .or_default()
                    .push(&group.fingerprint);
            }
        }
        rows.values_mut().for_each(|names| names.sort_unstable());
        rows
    }

    /// The counts, then a line per group with its first instance.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "diff: A {} ({}), B {} ({}); mode {}\n\
             diff: {} rows: {} identical, {} known, {} mismatched, {} not native\n",
            self.a.side,
            self.a.binary,
            self.b.side,
            self.b.binary,
            self.mode,
            self.rows,
            self.identical,
            self.known,
            self.mismatched,
            self.not_native
        );
        if self.rows > 0 && self.identical + self.known + self.mismatched == 0 {
            let _ = writeln!(
                text,
                "diff: warning: no row was compared, so this run shows nothing about the sides"
            );
        }
        for (side, reasons) in &self.refusals {
            let counts: Vec<String> = reasons
                .iter()
                .map(|(reason, rows)| format!("{reason} {}", rows.len()))
                .collect();
            let _ = writeln!(
                text,
                "diff: {side} declined natively: {}",
                counts.join(", ")
            );
        }
        for (side, counted) in &self.signatures {
            let Signatures {
                ok,
                bad,
                unverified,
            } = counted;
            let _ = writeln!(
                text,
                "diff: {side} signatures: {ok} ok, {bad} bad, {unverified} unverified"
            );
        }
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
