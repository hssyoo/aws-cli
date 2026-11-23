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
            builtins: &[],
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

const PAGINATOR_FIX: &str =
    "awscli.customizations.dynamodb.paginatorfix.register_dynamodb_paginator_fix";

/// With the client's real table, dynamodb's discovery block is covered
/// (`customizations::DYNAMODB`); what keeps its operations Python's is the
/// CLI's own: the paginator fix on its calls and the wizard commands on its
/// command table.
#[test]
fn dynamodb_is_uncovered_only_by_its_cli_customizations() -> Result<()> {
    with_models(|models| {
        let table = &customizations::TABLE[..];
        let found = matches(&models, "dynamodb", "describe-table");
        let missing: Vec<(&str, &str)> = found
            .iter()
            .filter(|one| {
                uncovered(
                    std::slice::from_ref(*one),
                    table,
                    "dynamodb",
                    "DescribeTable",
                )
                .is_some()
            })
            .map(|one| (&*one.pattern, &*one.name))
            .collect();
        assert_eq!(
            missing,
            [
                (
                    "building-command-table.dynamodb",
                    "awscli.customizations.wizard.commands.register_wizard_commands"
                ),
                ("calling-command.dynamodb.*", PAGINATOR_FIX),
            ]
        );
    })
}
