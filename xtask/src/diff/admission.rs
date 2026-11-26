//! Native admission. A side on the native route declines a command the
//! native implementation cannot run: it exits 255 having printed only
//! `aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable
//! (<reason>); Python fallback disabled`. Such a run is not native, and is
//! counted by reason, never compared.
//!
//! - Both sides decline alike, or only a native side facing another route
//!   (the oracle) declines: the row is not native, not a mismatch.
//! - Two native sides disagree (one declines, or the reasons differ): the
//!   admission changed, `admission-changed:a|b|reason`, which no known
//!   difference covers, as it changes which implementation users get.
//!
//! A row that is not compared still shows the other side's timeouts and
//! harness errors.

use std::collections::BTreeMap;

use super::capture::Capture;
use super::compare::Diff;
use super::corpus::Row;
use super::options::Route;
use super::{Run, Side, report};

const REFUSAL_START: &str =
    "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable (";
const REFUSAL_END: &str = "); Python fallback disabled\n";

/// Each side's refusals: the rows each reason declined.
pub type Refusals = BTreeMap<&'static str, BTreeMap<String, Vec<String>>>;

/// The reason `side` declined in `capture`, if it did.
pub fn refusal<'a>(side: &Side, capture: &'a Capture) -> Option<&'a str> {
    let quiet = capture.stdout.is_empty() && capture.requests.is_empty();
    if side.route != Route::Native || capture.exit != Some(255) || !quiet {
        return None;
    }
    let stderr = std::str::from_utf8(&capture.stderr).ok()?;
    let reason = stderr
        .strip_prefix(REFUSAL_START)?
        .strip_suffix(REFUSAL_END)?;
    let plain = !reason.is_empty() && !reason.contains(['(', ')', '\n']);
    plain.then_some(reason)
}

/// The differences of row `row`, or none when it is not native.
pub fn diffs(
    run: &Run,
    row: &Row,
    pair: (&Capture, &Capture),
    refusals: &mut Refusals,
) -> Option<Vec<Diff>> {
    let (a, b) = pair;
    let declined = (refusal(&run.a, a), refusal(&run.b, b));
    for (side, reason) in [("a", declined.0), ("b", declined.1)] {
        if let Some(reason) = reason {
            let rows = refusals
                .entry(side)
                .or_default()
                .entry(reason.to_owned())
                .or_default();
            rows.push(row.id.clone());
        }
    }
    let both_native = run.a.route == Route::Native && run.b.route == Route::Native;
    let changed = match declined {
        (None, None) => return Some(report::row_diffs(row, a, b)),
        (Some(x), Some(y)) if x == y => None,
        (Some(x), Some(y)) => Some(("reason", x, y)),
        (Some(x), None) => both_native.then_some(("a", x, "ran natively")),
        (None, Some(y)) => both_native.then_some(("b", "ran natively", y)),
    };
    let mut found = failures("a", a);
    found.extend(failures("b", b));
    if let Some((which, x, y)) = changed {
        found.push(diff(format!("admission-changed:{which}"), x, y));
    }
    (!found.is_empty()).then_some(found)
}

fn diff(fingerprint: String, a: &str, b: &str) -> Diff {
    Diff {
        fingerprint,
        key: None,
        request: None,
        a: a.to_owned(),
        b: b.to_owned(),
    }
}

/// A run's own timeout and harness errors, which show uncompared.
fn failures(side: &str, capture: &Capture) -> Vec<Diff> {
    let mut found: Vec<Diff> = capture
        .harness
        .iter()
        .map(|error| {
            let kind = error.split(':').next().unwrap_or_default();
            diff(format!("harness:{side}:{kind}"), error, "")
        })
        .collect();
    if capture.timed_out {
        found.push(diff(format!("timeout:{side}"), "", ""));
    }
    found
}

#[cfg(test)]
mod tests;
