//! A platform's baseline file, `tests/perf/baselines/<os>-<arch>.json`.
//!
//! For each tier it holds a [`Section`]: the tier's [`Calibration`], the
//! one place the gate's tolerances live, which `cargo xtask perf
//! --calibrate` writes, and the tier's stored record, without its runs,
//! which `--write-baseline` writes and `--check-baseline` judges against.
//! Each tier is calibrated and recorded in its own rounds: a series'
//! median depends on the series it runs among (short native calls ran 15
//! to 17% slower among the full tier's than among the quick tier's), so a
//! quick run is judged only against quick numbers.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::gate::Tolerances;
use super::record::{FORMAT, Host, Record, Tier};
use crate::{TaskError, deps};

/// How a tier's tolerances were derived, and what they are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Calibration {
    /// When the calibration finished, in Unix seconds.
    pub(super) unix_s: f64,
    /// The commit of the binary calibrated, when known.
    pub(super) commit: Option<String>,
    pub(super) host: Host,
    /// The least and greatest load average seen before its runs.
    pub(super) load_average: [f64; 2],
    /// How many comparisons of the binary with itself it ran.
    pub(super) repeats: u32,
    /// The tolerances of a comparison with a base binary.
    pub(super) comparison: Tolerances,
    /// The tolerances of a check against the stored record.
    pub(super) baseline: Tolerances,
}

/// One tier's part of the file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Section {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) calibration: Option<Calibration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) record: Option<Record>,
}

/// The baseline file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Baseline {
    pub(super) format: u32,
    #[serde(default)]
    pub(super) quick: Section,
    #[serde(default)]
    pub(super) full: Section,
}

impl Default for Baseline {
    fn default() -> Self {
        Self {
            format: FORMAT,
            quick: Section::default(),
            full: Section::default(),
        }
    }
}

impl Baseline {
    /// The file at `path`; `None` when there is none.
    pub(super) fn read(path: &Path) -> Result<Option<Self>, TaskError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, &path.display().to_string()).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// The file read from `text`, refusing another format or a section
    /// whose record is of the other tier.
    pub(super) fn parse(text: &str, origin: &str) -> Result<Self, TaskError> {
        let baseline: Self = serde_json::from_str(text)
            .map_err(|error| deps::error(format!("{origin} is not a perf baseline: {error}")))?;
        if baseline.format != FORMAT {
            return Err(deps::error(format!(
                "{origin} has format {}, expected {FORMAT}",
                baseline.format
            )));
        }
        for tier in [Tier::Quick, Tier::Full] {
            let record = baseline.section(tier).record.as_ref();
            if record.is_some_and(|record| record.format != FORMAT || record.tier != tier) {
                return Err(deps::error(format!(
                    "{origin}: its {0} record is not a format {FORMAT} {0} record",
                    tier.name()
                )));
            }
        }
        Ok(baseline)
    }

    pub(super) fn section(&self, tier: Tier) -> &Section {
        match tier {
            Tier::Quick => &self.quick,
            Tier::Full => &self.full,
        }
    }

    pub(super) fn section_mut(&mut self, tier: Tier) -> &mut Section {
        match tier {
            Tier::Quick => &mut self.quick,
            Tier::Full => &mut self.full,
        }
    }

    /// Write the file to `path`, creating its directory.
    pub(super) fn write(&self, path: &Path) -> Result<(), TaskError> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| deps::error(format!("perf baseline: {error}")))?;
        fs::write(path, text + "\n")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
