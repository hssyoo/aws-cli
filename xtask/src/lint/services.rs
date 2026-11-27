//! No service name in the generic code: what is particular to a service
//! lives in the `customizations` crate, keyed by model directory names, so
//! the generic crates serve any service the models describe.
//!
//! The names are the directories of botocore's data in the repository. A
//! string literal equal to one, in the non-test source of a [`SCOPE`]
//! entry, is a diagnostic unless [`EXEMPT`] gives that file and literal a
//! reason. Tests (`tests.rs`, `testing.rs`, and anything under a `tests` or
//! `testing` directory) may name services.
use super::{Diagnostic, Report, Rule, TaskError, lexical_tokens, read_source, token_line};
use std::path::Path;

/// The generic code, relative to the repository root: whole crates, and
/// of `cli-router` the generic model-command path and the settings both
/// paths share. `cli-router/src/lib.rs` is outside: it holds the s3
/// commands' own path (`S3_SERVICE`), which stays until s3 joins the
/// generic one.
const SCOPE: &[&str] = &[
    "crates/model/src",
    "crates/client/src",
    "crates/command/src",
    "crates/format/src",
    "crates/service-command/src",
    "crates/service-errors/src",
    "crates/cli-core/src",
    "crates/cli-router/src/generic.rs",
    "crates/cli-router/src/effective_config.rs",
    "crates/cli-router/src/effective_config",
];

/// Literals in scope that are a service's directory name but mean something
/// else, each by file and with why.
const EXEMPT: &[(&str, &str, &str)] = &[
    (
        "crates/client/src/protocol.rs",
        "ec2",
        "a wire protocol's name in botocore's priority list",
    ),
    (
        "crates/client/src/xml.rs",
        "amp",
        "the XML entity for `&`, which the reader decodes",
    ),
    (
        "crates/model/src/shapes/traits.rs",
        "location",
        "a model trait key, not the Location service",
    ),
    (
        "crates/model/src/frontend/c2j/traits.rs",
        "location",
        "a model trait key, not the Location service",
    ),
    (
        "crates/model/src/schema/projection.rs",
        "location",
        "a model trait key, not the Location service",
    ),
    (
        "crates/model/src/frontend/c2j/read.rs",
        "ec2",
        "a wire protocol's name, which names a protocol trait",
    ),
    (
        "crates/cli-core/src/client_config/mod.rs",
        "s3",
        "the s3 commands' own client configuration (`ClientConfig`, built for the S3 \
         client: its `S3_SERVICE` endpoint lookup and `s3` config section)",
    ),
    (
        "crates/cli-core/src/client_config/settings.rs",
        "s3",
        "the config file's `s3` section, which botocore reads for every client",
    ),
    (
        "crates/cli-router/src/effective_config.rs",
        "s3",
        "the config file's `s3` section, which botocore reads for every client",
    ),
    (
        "crates/cli-core/src/output.rs",
        "translate",
        "a struct field's name in its Debug rendering",
    ),
    (
        "crates/cli-core/src/shared_config.rs",
        "config",
        "the config file's name (`~/.aws/config`)",
    ),
];

/// botocore's data directory, whose subdirectories name the services.
const DATA: &str = "aws-cli/awscli/botocore/data";

/// The service-name diagnostics of `sources`, files under `root`.
pub(super) fn check(
    root: &Path,
    sources: &[std::path::PathBuf],
) -> Result<Vec<Diagnostic>, TaskError> {
    let names = service_names(&root.join(DATA))?;
    let mut report = Report::default();
    for path in sources {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if in_scope(&relative) {
            check_file(path, &relative, &read_source(path)?, &names, &mut report)?;
        }
    }
    Ok(report.diagnostics)
}

/// The service directory names, which the rule needs: a tree without them
/// fails rather than passing unchecked.
fn service_names(data: &Path) -> Result<Vec<String>, TaskError> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(data)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    if names.is_empty() {
        return Err(TaskError::Message(format!(
            "lint: no services under {DATA}"
        )));
    }
    Ok(names)
}

/// A non-test Rust source under a [`SCOPE`] entry.
fn in_scope(relative: &str) -> bool {
    let test = relative
        .split('/')
        .any(|part| matches!(part, "tests" | "testing" | "tests.rs" | "testing.rs"));
    relative.ends_with(".rs")
        && !test
        && SCOPE
            .iter()
            .any(|scope| relative == *scope || relative.starts_with(&format!("{scope}/")))
}

fn check_file(
    path: &Path,
    relative: &str,
    source: &str,
    names: &[String],
    report: &mut Report,
) -> Result<(), TaskError> {
    for token in lexical_tokens(source) {
        let Some(text) = literal_text(token) else {
            continue;
        };
        let exempt = EXEMPT
            .iter()
            .any(|(file, literal, _)| *file == relative && *literal == text);
        if !exempt && names.iter().any(|name| name == text) {
            let line = token_line(source, token);
            report.add(
                path,
                line,
                Rule::ServiceName,
                format!("names the service {text:?}"),
            )?;
        }
    }
    Ok(())
}

/// The text of a string literal token (`"…"`, `r#"…"#`, `b"…"`), or `None`
/// for any other token, a character literal included.
fn literal_text(token: &str) -> Option<&str> {
    let token = token.strip_prefix(['b', 'c']).unwrap_or(token);
    let token = match token.strip_prefix('r') {
        Some(raw) => raw.trim_matches('#'),
        None => token,
    };
    token.strip_prefix('"')?.strip_suffix('"')
}

#[cfg(test)]
mod tests;
