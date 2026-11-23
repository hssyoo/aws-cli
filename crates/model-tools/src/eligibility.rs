//! How many operations of the shipped services the native route runs: with
//! no arguments, with scalar arguments only, or why not. `cargo xtask
//! census` prints the counts and writes them to
//! `target/service-eligibility.txt`; nothing is committed.
use cli_core::cli_error::PythonReason;
use client::hooks::Customization;
use command::models::{ModelSource, Unpack};
use service_command::admit;
use std::collections::BTreeMap;

/// One operation's standing, before any argument is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Standing {
    /// Admitted with nothing supplied: no required argument.
    NoArguments,
    /// Admitted when its required arguments are given, all of which bind.
    ScalarArguments,
    /// A required argument only Python binds.
    RequiredNotScalar,
    /// Refused whatever the arguments.
    Refused(PythonReasonName),
}

/// A reason, ordered by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PythonReasonName(pub &'static str);

/// The standing of `[command, operation]`.
pub fn standing(
    source: &ModelSource<'_>,
    customizations: &[&'static Customization],
    command: &str,
    operation: &str,
) -> Standing {
    if let Err(reason) = admit::operation(source, customizations, command, operation) {
        return Standing::Refused(PythonReasonName(reason.name()));
    }
    let Some(table) = source.table(command, operation) else {
        return Standing::Refused(PythonReasonName(PythonReason::UnsupportedSubcommand.name()));
    };
    let mut required = table
        .arguments
        .iter()
        .filter(|argument| argument.required)
        .peekable();
    if required.peek().is_none() {
        return Standing::NoArguments;
    }
    if required.all(|argument| argument.unpack != Unpack::Deferred) {
        Standing::ScalarArguments
    } else {
        Standing::RequiredNotScalar
    }
}

/// Every root's operations, counted by standing.
pub fn census(
    source: &ModelSource<'_>,
    customizations: &[&'static Customization],
) -> BTreeMap<Standing, usize> {
    let mut counts = BTreeMap::new();
    let roots: Vec<String> = source
        .roots()
        .map(|roots| roots.keys().cloned().collect())
        .unwrap_or_default();
    for root in roots {
        let Some(service) = source.service(&root) else {
            continue;
        };
        for operation in service.operations.clone() {
            *counts
                .entry(standing(source, customizations, &root, &operation))
                .or_default() += 1;
        }
    }
    counts
}

/// The census as `cargo xtask census` prints it: the total, then each
/// standing's count.
pub fn report(counts: &BTreeMap<Standing, usize>) -> String {
    let total: usize = counts.values().sum();
    let mut report = format!("{total} operations\n");
    for (standing, count) in counts {
        report.push_str(&format!("{count:>6} {standing:?}\n"));
    }
    report
}

#[cfg(test)]
mod tests;
