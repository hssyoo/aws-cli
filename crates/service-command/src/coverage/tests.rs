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

/// Every row names an initializer the payload's registry lists under a
/// pattern the row is reached by: the table describes the AWS CLI the
/// fallback runs.
#[test]
fn every_row_is_a_registry_entry_under_its_pattern() -> Result<()> {
    with_models(|models| {
        let registry = &models.patterns().registry;
        for row in CLI {
            let listed = registry.iter().any(|(pattern, entries)| {
                row.reached.by(pattern) && entries.iter().any(|entry| entry.name() == row.entry)
            });
            assert!(listed, "{} under {:?}", row.entry, row.reached);
        }
    })
}

/// sts, lambda (whose table both editors edit), ec2 (argrename and
/// toplevelbool) and the fake models meet
/// only covered initializers, and every row is met by one of them: the
/// table is the cross-cutting set, no more.
#[test]
fn the_cross_cutting_initializers_are_covered_and_all_are_met() -> Result<()> {
    with_models(|models| {
        let mut met = Vec::new();
        for (command, operation) in [
            ("sts", "get-caller-identity"),
            ("lambda", "get-account-settings"),
            ("zquery", "get-thing"),
            ("zjson", "measure"),
            ("ec2", "describe-regions"),
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
                .any(|found| row.reached.by(&found.pattern) && found.name == row.entry);
            assert!(
                reached,
                "{} under {:?} is met by none",
                row.entry, row.reached
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
            retry_classifiers: &[],
            long_polling: false,
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

/// The rows reached through root table events are the initializers the
/// pack recorded as table editors, each listed under root table events
/// alone: the rows describe the AWS CLI the fallback runs, and the pack's
/// list of editors and the table's cannot drift apart.
#[test]
fn the_editor_rows_are_the_recorded_editors_listed_under_root_tables() -> Result<()> {
    with_models(|models| {
        let patterns = models.patterns();
        let editors = CLI.iter().filter(|row| row.reached == Reached::RootTable);
        for recorded in &patterns.table_editors {
            assert!(
                editors
                    .clone()
                    .any(|row| row.entry == recorded.entry.name()),
                "{} has no row",
                recorded.entry.name()
            );
        }
        for row in editors {
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
        let (case, acting) = routed(row.entry)?;
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

/// Every router refusal a client customization's `Inert` basis claims is
/// what refuses an invocation the botocore handler would act on, one that
/// otherwise routes native, as for the CLI's rows. A trait refusal is the
/// client's own, which the `customizations` crate checks.
#[test]
fn every_core_router_refusal_refuses_where_the_customization_acts() -> Result<()> {
    let mut checked = 0;
    for customization in customizations::TABLE {
        for (name, basis) in customization.replaces {
            let Basis::Inert {
                refused_as: Refusal::Router(reason),
                ..
            } = basis
            else {
                continue;
            };
            let (_, acting) = routed(name)?;
            assert!(
                matches!(acting, Verdict::Python(found) if found == *reason),
                "{name}: {acting:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no router refusal was checked");
    Ok(())
}

/// argrename's row is reached by exactly the patterns the registry lists
/// it under, each of which is one of its maps' event portions: the port
/// covers every service the registry initializes it for.
#[test]
fn the_rename_row_is_reached_by_the_registry_s_patterns_alone() -> Result<()> {
    with_models(|models| {
        let row = CLI
            .iter()
            .find(|row| row.reached == Reached::Renames)
            .unwrap_or_else(|| panic!("no rename row"));
        let listed: Vec<&str> = models
            .patterns()
            .registry
            .iter()
            .filter(|(_, entries)| entries.iter().any(|entry| entry.name() == row.entry))
            .map(|(pattern, _)| &**pattern)
            .collect();
        assert!(listed.len() > 50, "{listed:?}");
        assert!(
            listed.iter().all(|pattern| row.reached.by(pattern)),
            "{listed:?}"
        );
        for portion in command::models::argedits::rename_portions() {
            let pattern = format!("building-argument-table.{portion}");
            assert!(
                listed.contains(&pattern.as_str()),
                "{pattern} is not listed"
            );
        }
        for pattern in [
            "building-argument-table.ec2",
            "building-argument-table.*",
            "building-argument-table",
            "building-argument-table.ec2.describe-regions",
            "building-command-table.ec2",
            "operation-args-parsed.ec2.*",
        ] {
            assert!(!row.reached.by(pattern), "{pattern}");
        }
    })
}

/// `ParameterAlias('Filter', 'Filters')` acts only on params holding
/// `Filters` for an input with a `Filter` member; no ec2 input has both,
/// so no parse of the arguments the derivation gives can hold both.
#[test]
fn no_ec2_input_has_both_filter_members() -> Result<()> {
    with_models(|models| {
        let service = models.service("ec2").unwrap_or_else(|| panic!("ec2"));
        let mut with_filter = 0;
        for operation in service.model.schema().service().operations() {
            let Some(input) = operation.input() else {
                continue;
            };
            let has = |name: &str| {
                input
                    .members()
                    .any(|member| member.member_name() == Some(name))
            };
            if has("Filter") {
                with_filter += 1;
                assert!(!has("Filters"), "{}", operation.name());
            }
        }
        assert!(with_filter > 0, "no ec2 input has Filter");
    })
}

/// The case of `entry`, whose native invocation is checked to route
/// native, and the verdict of its acting one.
fn routed(entry: &str) -> Result<(&'static Case, Verdict)> {
    let case = case(entry).unwrap_or_else(|| panic!("no case for {entry}"));
    let native = verdict(case.native, "", "")?;
    assert!(matches!(native, Verdict::Run(_)), "{entry}: {native:?}");
    let acting = verdict(case.acting, case.config, case.alias)?;
    Ok((case, acting))
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
/// parse failures and those of the tables the editors alone edit, and not
/// those of a table another initializer edits.
#[test]
fn the_table_s_edits_leave_only_other_customized_tables_to_python() -> Result<()> {
    with_models(|models| {
        assert!(models.reproduces_failures("sts", None));
        assert!(models.reproduces_failures("sts", Some("get-caller-identity")));
        assert!(models.reproduces_failures("zjson", Some("get-thing")));
        for command in ["lambda", "ses", "ec2", "dynamodb", "iam", "events"] {
            assert!(models.reproduces_failures(command, None), "{command}");
        }
        assert!(models.reproduces_failures("lambda", Some("get-account-settings")));
        assert!(
            !models.reproduces_failures("logs", None),
            "register_logs_commands adds logs' hand-written commands"
        );
        assert!(
            !models.reproduces_failures("emr", Some("list-clusters")),
            "emr_initialize edits emr's table"
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

/// The case of each `Inert` basis with a router refusal or a CLI row, by
/// the end of its entry.
const CASES: &[(&str, Case)] = &[
    (
        "._set_auth_scheme_preference_signer",
        with_files("[default]\nauth_scheme_preference = sigv4\n", ""),
    ),
    (
        ".add_generate_presigned_url",
        with_argv(IDENTITY, &["eks", "get-token", "--cluster-name", "c"]),
    ),
    (
        ".alias_parameter_in_call",
        with_argv(
            &["ec2", "describe-regions"],
            &[
                "ec2",
                "describe-flow-logs",
                "--cli-input-json",
                r#"{"Filters": []}"#,
            ],
        ),
    ),
    (
        ".register_cli_input_args",
        with_argv(
            IDENTITY,
            &["sts", "get-caller-identity", "--cli-input-json", "{}"],
        ),
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
        let schema = service.model.schema().service();
        let endpoints = EndpointData::load(models.search(), &service.name, schema).ok()?;
        let entry = service.paginator(found.name()).map(|entry| &entry.config);
        Some(client::admission::static_refusal(
            &service.model,
            found,
            &endpoints,
            &service.name,
            &customizations::TABLE,
            entry,
        ))
    })
    .map(Option::flatten)
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
