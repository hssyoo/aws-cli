#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::testing::{Result, replacing_all, with_models};
use command::models::ModelSource;

fn matches(models: &ModelSource<'_>, command: &str, operation: &str) -> Vec<Match> {
    models
        .matches(command, operation)
        .unwrap_or_else(|| panic!("{command} {operation}"))
}

/// Every row names an initializer the payload's registry lists under the
/// row's pattern: the table describes the AWS CLI the fallback runs.
#[test]
fn every_row_is_a_registry_entry_under_its_pattern() -> Result<()> {
    with_models(|models| {
        let registry = &models.patterns().registry;
        for row in CLI {
            let listed = registry.iter().any(|(pattern, entries)| {
                &**pattern == row.pattern && entries.iter().any(|entry| entry.name() == row.entry)
            });
            assert!(listed, "{} under {}", row.entry, row.pattern);
        }
    })
}

/// sts and the fake models meet only covered initializers, and every row
/// is met by one of them: the table is the cross-cutting set, no more.
#[test]
fn the_cross_cutting_initializers_are_covered_and_all_are_met() -> Result<()> {
    with_models(|models| {
        let mut met = Vec::new();
        for (command, operation) in [
            ("sts", "get-caller-identity"),
            ("zquery", "get-thing"),
            ("zjson", "measure"),
        ] {
            let found = matches(&models, command, operation);
            let everything = replacing_all(&models, command, operation);
            assert_eq!(
                uncovered(&found, everything, command, operation),
                None,
                "{command}"
            );
            met.extend(
                found
                    .into_iter()
                    .filter(|found| found.kind == MatchKind::Registry),
            );
        }
        for row in CLI {
            let reached = met
                .iter()
                .any(|found| found.pattern == row.pattern && found.name == row.entry);
            assert!(
                reached,
                "{} under {} is met by none",
                row.entry, row.pattern
            );
        }
    })
}

/// An initializer reached through a service's own pattern is uncovered
/// though the same one, reached through a shared event, is covered; and a
/// botocore row is covered only by a customization in scope.
#[test]
fn coverage_is_per_pattern_and_per_scope() -> Result<()> {
    with_models(|models| {
        let found = matches(&models, "cloudfront", "create-invalidation");
        let everything = replacing_all(&models, "cloudfront", "create-invalidation");
        let missing = uncovered(&found, everything, "cloudfront", "CreateInvalidation")
            .unwrap_or_else(|| panic!("covered"));
        assert_eq!(missing.name, "awscli.customizations.cloudfront.register");
        assert_ne!(missing.pattern, "top-level-args-parsed");
        let found = matches(&models, "sts", "get-caller-identity");
        let missing =
            uncovered(&found, &[], "sts", "GetCallerIdentity").unwrap_or_else(|| panic!("covered"));
        assert_ne!(missing.kind, MatchKind::Registry);
        let everything = replacing_all(&models, "sts", "get-caller-identity");
        let elsewhere = Customization {
            id: "elsewhere",
            scope: client::hooks::Scope::service("zquery"),
            setting_defaults: &[],
            interceptors: &[],
            replaces: everything[0].replaces,
        };
        assert_eq!(
            uncovered(&found, everything, "sts", "GetCallerIdentity"),
            None
        );
        assert!(uncovered(&found, &[&elsewhere], "sts", "GetCallerIdentity").is_some());
    })
}
