//! The crate layering rule: the workspace's crates stand in layers, from
//! the leaves (L0) up to the binary (L6), and a crate may depend only on
//! crates in lower layers, plus the few siblings its row names. So the
//! model, the formatter and the command API never pull in the network
//! client, nothing below the commands pulls in a command or the router,
//! and only the client among the dynamic-client crates reaches the CRT. A
//! workspace crate's normal and build dependencies on other workspace
//! crates must appear in its row of [`ALLOWED`], and every workspace crate
//! must have a row.
//!
//! Dev-dependencies are not checked: a crate's tests may assemble layers
//! above it (the parity test renders `client`'s results with `format`).
//! Which source may name a service is the no-service-name test's rule, not
//! this one.
use super::{Diagnostic, Report, Rule, TaskError, read_source};
use std::path::Path;

/// Each workspace crate and the workspace crates it may depend on, by layer.
/// Dependencies point down, so a row names only lower layers, plus the
/// siblings it lists. Only `client` among the crates added for the dynamic
/// client may depend on `crt`; `command` and `format` never depend on
/// `client`; `credentials` never does; and nothing below the commands may
/// depend on `s3`, `cli-router`, `aws-cli` or `python-fallback`.
const ALLOWED: &[(&str, &[&str])] = &[
    // L0: leaves.
    ("awscli-argparse", &["pytext"]),
    ("awscli-config", &[]),
    ("build-support", &[]),
    ("crt-sys", &["build-support"]),
    ("numa", &[]),
    ("pyload", &["sha256"]),
    // CPython's text primitives, one implementation each.
    ("pytext", &[]),
    ("sha256", &[]),
    ("sigv4", &["sha256", "timestamp"]),
    ("timestamp", &["pytext"]),
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
    ("model", &["pyload", "pytext", "timestamp"]),
    // L2: client and format.
    ("client", CLIENT),
    (
        "format",
        &["awscli-config", "cli-core", "model", "timestamp"],
    ),
    // L3: the command API, over format and the model, never the client.
    ("command", COMMAND),
    // The CLI's rendering of a call's errors (`errorhandler.py`): over the
    // client's errors, format and the command API's floor, never a command
    // crate.
    ("service-errors", SERVICE_ERRORS),
    // L4: the command crates. `customizations` holds the per-service
    // edits and hooks, over the client and the command API; what it needs
    // of `s3` goes through a trait in `command`, not a dependency.
    ("s3", S3),
    ("customizations", CUSTOMIZATIONS),
    // The generic service command: admission, binding and the call, over
    // the command API, the client and the CLI's error rendering.
    ("service-command", SERVICE_COMMAND),
    // L5: routing. Its `s3` and `command` edges stay only while the router,
    // not the binary, assembles the registry of native commands.
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
    "pytext",
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
    "pytext",
    "timestamp",
];
const CUSTOMIZATIONS: &[&str] = &[
    "awscli-config",
    "cli-core",
    "client",
    "command",
    "format",
    "model",
    "pytext",
    "timestamp",
];
const SERVICE_ERRORS: &[&str] = &[
    "awscli-config",
    "cli-core",
    "client",
    "credentials",
    "format",
    "model",
    "pytext",
    "timestamp",
];
const SERVICE_COMMAND: &[&str] = &[
    "awscli-argparse",
    "cli-core",
    "client",
    "command",
    "credentials",
    "format",
    "model",
    "service-errors",
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
    "pytext",
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
    "customizations",
    "pyload",
    "python-fallback",
    "s3",
    "service-command",
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
                let detail = format!(
                    "{name} may not depend on {on}: a crate depends only on lower layers \
                     (leaves; cli-core, credentials, crt, model; client, format; the command \
                     API; the commands; routing; the binary), as `ALLOWED` in \
                     xtask/src/lint/layers.rs lists them"
                );
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
