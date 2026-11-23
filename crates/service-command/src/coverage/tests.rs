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

const PAGINATOR_FIX: &str =
    "awscli.customizations.dynamodb.paginatorfix.register_dynamodb_paginator_fix";

/// With the client's real table, dynamodb's discovery block is covered
/// (`customizations::DYNAMODB`), and so are the wizard commands on its
/// command table; what keeps its operations Python's is the CLI's own
/// paginator fix on its calls.
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
        assert_eq!(missing, [("calling-command.dynamodb.*", PAGINATOR_FIX)]);
    })
}

const REMOVALS: &str = "awscli.customizations.removals.register_removals";
const WIZARDS: &str = "awscli.customizations.wizard.commands.register_wizard_commands";

/// Every editor row names an initializer the pack recorded as a table
/// editor, listed under root table events alone: the rows describe the AWS
/// CLI the fallback runs.
#[test]
fn every_editor_row_is_a_recorded_editor_listed_under_root_tables() -> Result<()> {
    with_models(|models| {
        let patterns = models.patterns();
        for row in TABLE_EDITORS {
            assert!(
                patterns
                    .table_editors
                    .iter()
                    .any(|editor| editor.entry.name() == row.entry),
                "{} is not recorded",
                row.entry
            );
            let listed: Vec<&str> = patterns
                .registry
                .iter()
                .filter(|(_, entries)| entries.iter().any(|entry| entry.name() == row.entry))
                .map(|(pattern, _)| &**pattern)
                .collect();
            assert!(!listed.is_empty(), "{}", row.entry);
            assert!(
                listed.iter().all(|pattern| root_table_event(pattern)),
                "{}: {listed:?}",
                row.entry
            );
        }
    })
}

/// lambda's operations meet both editors through lambda's own table
/// event, and with the client's side settled nothing else of the CLI's is
/// left uncovered for one without a paginator.
#[test]
fn the_editors_are_covered_through_a_root_s_table_event() -> Result<()> {
    with_models(|models| {
        let found = matches(&models, "lambda", "get-account-settings");
        for entry in [REMOVALS, WIZARDS] {
            assert!(
                found
                    .iter()
                    .any(|one| one.pattern == "building-command-table.lambda" && one.name == entry),
                "{entry}"
            );
        }
        let everything = replacing_all(&models, "lambda", "get-account-settings");
        assert_eq!(
            uncovered(&found, everything, "lambda", "GetAccountSettings"),
            None
        );
    })
}

/// An editor is covered through a root's own table event alone: not the
/// main table's, a subcommand's, a wildcard or any other event.
#[test]
fn an_editor_through_any_other_pattern_is_uncovered() {
    let registry = |pattern: &str, name: &str| Match {
        kind: MatchKind::Registry,
        pattern: pattern.to_owned(),
        name: name.to_owned(),
    };
    for pattern in [
        "building-command-table.ses",
        "building-command-table.lexv2-runtime",
    ] {
        let found = [registry(pattern, REMOVALS)];
        assert_eq!(uncovered(&found, &[], "ses", "Op"), None, "{pattern}");
    }
    for pattern in [
        "building-command-table",
        "building-command-table.main",
        "building-command-table.*",
        "building-command-table.lambda_invoke",
        "building-command-table.lambda.invoke",
        "building-command-table.",
        "session-initialized",
    ] {
        let found = [registry(pattern, WIZARDS)];
        assert!(
            uncovered(&found, &[], "lambda", "Op").is_some(),
            "{pattern}"
        );
    }
    let other = [registry(
        "building-command-table.logs",
        "awscli.customizations.logs.register_logs_commands",
    )];
    assert!(uncovered(&other, &[], "logs", "Op").is_some());
}
