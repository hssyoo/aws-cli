//! The crate layering rule of the plan's layer diagram (`plan/PLAN.md` §1.2,
//! dependency rules §2.2): a workspace crate's normal and build
//! dependencies on other workspace crates must appear in its row of
//! [`ALLOWED`], and every workspace crate must have a row.
//!
//! Dev-dependencies are not checked: a crate's tests may assemble layers
//! above it (the parity test renders `client`'s results with `format`).
//! Which source may name a service is the no-service-name test's rule, not
//! this one.
use super::{Diagnostic, Report, Rule, TaskError, read_source};
use std::path::Path;

/// Each workspace crate and the workspace crates it may depend on, by layer.
/// Arrows point down, so a row names only lower layers, plus the siblings
/// the diagram draws. Only `client` among the crates added for the dynamic
/// client may depend on `crt`; `command` and `format` never depend on
/// `client`; `credentials` never does; and nothing below the commands may
/// depend on `s3`, `cli-router`, `aws-cli` or `python-fallback`.
const ALLOWED: &[(&str, &[&str])] = &[
    // L0: leaves.
    ("awscli-argparse", &[]),
    ("awscli-config", &[]),
    ("build-support", &[]),
    ("crt-sys", &["build-support"]),
    ("numa", &[]),
    ("pyload", &["sha256"]),
    ("sha256", &[]),
    ("sigv4", &["sha256", "timestamp"]),
    ("timestamp", &[]),
    // L1: credentials, model, crt and cli-core; `cli-core` is the common
    // floor the other three may stand on.
    ("cli-core", &["awscli-argparse", "awscli-config"]),
    (
        "credentials",
        &["awscli-config", "cli-core", "sha256", "sigv4", "timestamp"],
    ),
    (
        "crt",
        &[
            "cli-core",
            "crt-sys",
            "numa",
            "sha256",
            "sigv4",
            "timestamp",
        ],
    ),
    ("model", &["pyload", "timestamp"]),
    // L2: client and format.
    ("client", CLIENT),
    (
        "format",
        &["awscli-config", "cli-core", "model", "timestamp"],
    ),
    // L3: the command API, over format and the model, never the client.
    ("command", COMMAND),
    // L4: the command crates.
    ("s3", S3),
    // L5: routing. Its `s3` and `command` edges are the arms the plan's
    // §2.2 rule 2 removes when the binary assembles the registry.
    ("cli-router", ROUTER),
    // L6: the binary and the embedded interpreter.
    ("aws-cli", BINARY),
    ("python-fallback", &["build-support", "pyload"]),
    ("xtask", &["build-support"]),
];

const CLIENT: &[&str] = &[
    "awscli-config",
    "cli-core",
    "credentials",
    "crt",
    "model",
    "sha256",
    "sigv4",
    "timestamp",
];
const COMMAND: &[&str] = &[
    "awscli-argparse",
    "awscli-config",
    "cli-core",
    "format",
    "model",
    "pyload",
    "timestamp",
];
const S3: &[&str] = &[
    "awscli-argparse",
    "awscli-config",
    "cli-core",
    "credentials",
    "crt",
    "model",
    "numa",
    "pyload",
    "sigv4",
];
const ROUTER: &[&str] = &[
    "awscli-argparse",
    "awscli-config",
    "cli-core",
    "command",
    "credentials",
    "s3",
];
const BINARY: &[&str] = &[
    "awscli-argparse",
    "awscli-config",
    "build-support",
    "cli-core",
    "cli-router",
    "command",
    "crt",
    "crt-sys",
    "pyload",
    "python-fallback",
    "s3",
];

/// The layering diagnostics for `cargo metadata`'s workspace `packages`.
pub(super) fn check(metadata: &serde_json::Value) -> Result<Vec<Diagnostic>, TaskError> {
    let mut report = Report::default();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let name = package["name"].as_str().unwrap_or_default();
        let manifest = Path::new(package["manifest_path"].as_str().unwrap_or_default());
        let Some((_, allowed)) = ALLOWED.iter().find(|(crate_name, _)| *crate_name == name) else {
            let detail = format!("crate {name} has no row in the layer table");
            report.add(manifest, 0, Rule::Layering, detail)?;
            continue;
        };
        for dependency in package["dependencies"].as_array().into_iter().flatten() {
            let on = dependency["name"].as_str().unwrap_or_default();
            let checked = dependency["kind"].as_str() != Some("dev");
            if checked && dependency["path"].is_string() && !allowed.contains(&on) {
                let key = dependency["rename"].as_str().unwrap_or(on);
                let line = declared_line(manifest, key)?;
                let detail = format!("{name} may not depend on {on} (plan/PLAN.md §1.2, §2.2)");
                report.add(manifest, line, Rule::Layering, detail)?;
            }
        }
    }
    Ok(report.diagnostics)
}

/// The line declaring dependency `key` in `manifest`, or zero when the
/// manifest cannot be read or does not spell it as `key = …`.
fn declared_line(manifest: &Path, key: &str) -> Result<usize, TaskError> {
    if !manifest.is_file() {
        return Ok(0);
    }
    let text = read_source(manifest)?;
    let declares = |line: &str| {
        line.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with(['=', '.']))
    };
    Ok(text.lines().position(declares).map_or(0, |index| index + 1))
}

#[cfg(test)]
mod tests;
