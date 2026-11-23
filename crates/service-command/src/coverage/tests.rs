#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::testing::{Result, files, replacing_all, route, with_models};
use cli_router::Verdict;
use client::admission::Unsupported;
use client::endpoint::EndpointData;
use command::OperationSource;
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

/// Every `Implemented` row names an item the workspace defines.
#[test]
fn every_implemented_path_is_defined() -> Result<()> {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for row in CLI {
        if let Basis::Implemented { at } = row.basis {
            assert!(
                client::testing::defines(&workspace, at)?,
                "{at} ({})",
                row.entry
            );
        }
    }
    Ok(())
}

/// Every `Inert` row's refusal is what refuses an invocation its
/// initializer would act on, one that otherwise routes native: the
/// router's verdict, or the client's static refusal (which the router may
/// precede with one of its own; the invocation is Python's either way).
#[test]
fn every_inert_refusal_refuses_where_the_initializer_acts() -> Result<()> {
    for row in CLI {
        let Basis::Inert { refused_as, .. } = row.basis else {
            continue;
        };
        let case = case(row.entry).unwrap_or_else(|| panic!("no case for {}", row.entry));
        let native = verdict(case.native, "", "")?;
        assert!(
            matches!(native, Verdict::Run(_)),
            "{}: {native:?}",
            row.entry
        );
        let acting = verdict(case.acting, case.config, case.alias)?;
        match refused_as {
            Refusal::Router(reason) => assert!(
                matches!(acting, Verdict::Python(found) if found == reason),
                "{}: {acting:?}",
                row.entry
            ),
            Refusal::Trait(key) => {
                assert_trait_refused(row.entry, key, case.acting)?;
                assert!(
                    matches!(acting, Verdict::Python(_)),
                    "{}: {acting:?}",
                    row.entry
                );
            }
        }
    }
    Ok(())
}

/// The client refuses the operation `acting` names as carrying `key`.
fn assert_trait_refused(entry: &str, key: &str, acting: &[&str]) -> Result<()> {
    let [command, operation, ..] = acting else {
        panic!("{entry}: no operation");
    };
    let refused = static_refusal(command, operation)?;
    let expected = Unsupported::Trait(key.to_owned());
    assert_eq!(refused.as_ref(), Some(&expected), "{entry}");
    Ok(())
}

/// With the table's edits reproduced, the derivation reproduces sts's
/// parse failures, and not those of a table another initializer edits.
#[test]
fn the_table_s_edits_leave_only_other_customized_tables_to_python() -> Result<()> {
    with_models(|models| {
        assert!(models.reproduces_failures("sts", None));
        assert!(models.reproduces_failures("sts", Some("get-caller-identity")));
        assert!(models.reproduces_failures("zjson", Some("get-thing")));
        assert!(
            !models.reproduces_failures("ec2", None),
            "removals edit ec2's table"
        );
    })
}

/// Two invocations over the fake models: `native` routes native, and
/// `acting` is one the initializer acts on, with `config` and `alias` as
/// the shared config and alias files.
struct Case {
    native: &'static [&'static str],
    acting: &'static [&'static str],
    config: &'static str,
    alias: &'static str,
}

const IDENTITY: &[&str] = &["sts", "get-caller-identity"];
const GET_THING: &[&str] = &["zjson", "get-thing", "--thing-id", "a"];

const fn with_files(config: &'static str, alias: &'static str) -> Case {
    Case {
        native: IDENTITY,
        acting: IDENTITY,
        config,
        alias,
    }
}

const fn with_argv(native: &'static [&'static str], acting: &'static [&'static str]) -> Case {
    Case {
        native,
        acting,
        config: "",
        alias: "",
    }
}

/// The case of each `Inert` row, by the end of its entry.
const CASES: &[(&str, Case)] = &[
    (
        ".register_cli_input_args",
        with_argv(
            IDENTITY,
            &["sts", "get-caller-identity", "--cli-input-json", "{}"],
        ),
    ),
    (
        ".register_pagination",
        with_argv(&["zjson", "ping"], &["zjson", "list-things"]),
    ),
    (
        ".register_generate_cli_skeleton",
        with_argv(
            IDENTITY,
            &["sts", "get-caller-identity", "--generate-cli-skeleton"],
        ),
    ),
    (
        ".register_streaming_output_arg",
        with_argv(&["zjson", "ping"], &["zjson", "fetch-blob", "out"]),
    ),
    (
        ".register_alias_commands",
        with_files("", "[command sts]\nwhoami = get-caller-identity\n"),
    ),
    (
        ".register_init_binary_formatter",
        with_argv(
            GET_THING,
            &["zjson", "get-thing", "--thing-id", "a", "--payload", "aGk="],
        ),
    ),
    (
        ".register_assume_role_provider",
        with_files(
            "[default]\nrole_arn = arn:aws:iam::123456789012:role/r\n",
            "",
        ),
    ),
    (
        ".register_history_mode",
        with_files("[default]\ncli_history = enabled\n", ""),
    ),
    (
        ".register_sso_commands",
        with_files(
            "[default]\nsso_start_url = https://example.invalid/start\n",
            "",
        ),
    ),
    (
        "cloudfront.register",
        with_argv(IDENTITY, &["cloudfront", "list-distributions"]),
    ),
    (
        ".register_param_shorthand_parser",
        with_argv(
            GET_THING,
            &[
                "zjson",
                "get-thing",
                "--thing-id",
                "a",
                "--attributes",
                "k=v",
            ],
        ),
    ),
];

fn case(entry: &str) -> Option<&'static Case> {
    CASES
        .iter()
        .find(|(end, _)| entry.ends_with(end))
        .map(|(_, case)| case)
}

/// The router's verdict for `argv` with `config` and `alias`, and the
/// environment's keys.
fn verdict(argv: &[&str], config: &str, alias: &str) -> Result<Verdict> {
    let keys = [("AWS_ACCESS_KEY_ID", "a"), ("AWS_SECRET_ACCESS_KEY", "b")];
    route(argv, &keys, &files(config, alias))
}

/// The client's static refusal of `[command, operation]`.
fn static_refusal(command: &str, operation: &str) -> Result<Option<Unsupported>> {
    with_models(|models| {
        let service = models.service(command)?;
        let found = service.operation(operation)?;
        let endpoints =
            EndpointData::load(models.search(), &service.name, service.model.metadata()).ok()?;
        Some(client::admission::static_refusal(
            &service.model,
            found,
            &endpoints,
            &service.name,
            &customizations::TABLE,
        ))
    })
    .map(Option::flatten)
}
