//! The corpus: JSON Lines files, one invocation per line.
//!
//! A row has an `id` unique across the files read, the `source` it was
//! made from, the CLI's `argv` after `aws`, `env` (variables over the base
//! environment; a null value removes one), `data_path` (AWS_DATA_PATH,
//! relative to the repository root, for fake models), the `responses` the
//! stub gives the run's requests in order (the last repeating), each a
//! `status`, `headers` as ordered pairs, and a `body` or `body_base64`, and
//! an optional `timeout_s`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{TaskError, deps};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub argv: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub data_path: Option<PathBuf>,
    pub responses: Vec<Reply>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u32>,
}

impl Row {
    /// The texts the row itself gives the CLI: its arguments and variables.
    pub fn inputs(&self) -> impl Iterator<Item = &str> {
        let values = self.env.values().filter_map(Option::as_deref);
        self.argv.iter().map(String::as_str).chain(values)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    #[serde(default = "ok")]
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

fn ok() -> u16 {
    200
}

/// The `*.jsonl` files of `directory`, relative to `root`, in name order.
pub fn files(root: &Path, directory: &Path) -> Result<Vec<PathBuf>, TaskError> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root.join(directory))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            found.extend(path.file_name().map(|name| directory.join(name)));
        }
    }
    found.sort();
    Ok(found)
}

/// The rows of `files` whose id contains `filter`, in file order.
pub fn load(root: &Path, files: &[PathBuf], filter: Option<&str>) -> Result<Vec<Row>, TaskError> {
    let mut rows = Vec::new();
    for file in files {
        rows.extend(parse(
            &file.display().to_string(),
            &fs::read_to_string(root.join(file))?,
        )?);
    }
    let mut ids = BTreeSet::new();
    if let Some(row) = rows.iter().find(|row| !ids.insert(row.id.as_str())) {
        return Err(deps::error(format!(
            "corpus row id {} is not unique",
            row.id
        )));
    }
    rows.retain(|row| filter.is_none_or(|text| row.id.contains(text)));
    if rows.is_empty() {
        return Err(deps::error("no corpus row was selected"));
    }
    Ok(rows)
}

/// The rows of one file, `name` naming it in errors.
pub fn parse(name: &str, text: &str) -> Result<Vec<Row>, TaskError> {
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let at = |detail: String| deps::error(format!("{name}:{}: {detail}", index + 1));
        let row: Row = serde_json::from_str(line).map_err(|error| at(error.to_string()))?;
        if row.argv.is_empty() {
            return Err(at(format!("row {} has no argv", row.id)));
        }
        if row
            .responses
            .iter()
            .any(|reply| reply.body.is_some() && reply.body_base64.is_some())
        {
            return Err(at(format!(
                "a reply of row {} has both body and body_base64",
                row.id
            )));
        }
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
