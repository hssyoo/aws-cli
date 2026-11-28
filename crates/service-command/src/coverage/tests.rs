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

/// An operation of each service a command adder edits the table of, which
/// meets no other uncovered initializer: emr's meet its parser hook, so
/// emr's row is met through [`EMR`] alone.
const ADDED_TO: &[(&str, &str)] = &[
    ("cloudformation", "list-stacks"),
    ("cloudfront", "list-distributions"),
    ("cloudtrail", "describe-trails"),
    ("codeartifact", "list-domains"),
    ("codecommit", "list-repositories"),
    ("deploy", "list-applications"),
    ("configservice", "describe-config-rules"),
    ("datapipeline", "list-pipelines"),
    ("dlm", "get-lifecycle-policies"),
    ("dsql", "list-clusters"),
    ("ec2-instance-connect", "send-ssh-public-key"),
    ("ecr", "describe-repositories"),
    ("ecr-public", "describe-registries"),
    ("ecs", "list-clusters"),
    ("eks", "list-clusters"),
    ("emr-containers", "list-virtual-clusters"),
    ("gamelift", "list-builds"),
    ("lambda-web", "list-web-functions"),
    ("lightsail", "get-regions"),
    ("logs", "describe-log-groups"),
    ("rds", "describe-db-instances"),
    ("servicecatalog", "list-portfolios"),
    ("ssm", "describe-parameters"),
    ("sso", "list-accounts"),
];
const EMR: (&str, &str) = ("emr", "list-instances");
const EMR_PARSER: &str = "before-building-argument-table-parser.emr.*";

/// sts, lambda (whose table both editors edit), ec2 (argrename and
/// toplevelbool), the fake models and an operation of each command adder's
/// service meet only covered initializers, and every row is met by one of
/// them or by [`EMR`]: the table is the cross-cutting set and the command
/// adders, no more.
#[test]
fn the_cross_cutting_initializers_are_covered_and_all_are_met() -> Result<()> {
    with_models(|models| {
        let mut met = Vec::new();
        let shared = [
            ("sts", "get-caller-identity"),
            ("lambda", "get-account-settings"),
            ("zquery", "get-thing"),
            ("zjson", "measure"),
            ("ec2", "describe-regions"),
        ];
        for (command, operation) in shared.into_iter().chain(ADDED_TO.iter().copied()) {
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
        let (command, operation) = EMR;
        let found = matches(&models, command, operation);
        let everything = replacing_all(&models, command, operation);
        let missing = uncovered(&found, everything, command, "ListInstances")
            .unwrap_or_else(|| panic!("emr is covered"));
        assert_eq!(missing.pattern, EMR_PARSER);
        met.extend(
            found
                .into_iter()
                .filter(|found| found.kind == MatchKind::Registry && found.pattern != EMR_PARSER),
        );
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

/// The editors the registry also lists under patterns other than root
/// table events, with exactly those patterns: what each registers there is
/// not recorded, and no row reaches it through them.
const MIXED: &[(&str, &[&str])] = &[
    (
        "awscli.customizations.cloudfront.register",
        &[
            "building-argument-table.cloudfront.create-distribution",
            "building-argument-table.cloudfront.create-invalidation",
            "building-argument-table.cloudfront.update-distribution",
            "operation-args-parsed.cloudfront.create-distribution",
            "operation-args-parsed.cloudfront.create-invalidation",
            "operation-args-parsed.cloudfront.update-distribution",
            "top-level-args-parsed",
        ],
    ),
    (
        "awscli.customizations.codedeploy.codedeploy.register_codedeploy",
        &[
            "building-argument-table.deploy.create-deployment",
            "building-argument-table.deploy.get-application-revision",
            "building-argument-table.deploy.register-application-revision",
        ],
    ),
    (
        "awscli.customizations.datapipeline.register_customizations",
        &[
            "after-call.data-pipeline.GetPipelineDefinition",
            "building-argument-table.datapipeline.activate-pipeline",
            "building-argument-table.datapipeline.put-pipeline-definition",
            "doc-output.datapipeline.get-pipeline-definition",
        ],
    ),
    (
        "awscli.customizations.emr.emr.emr_initialize",
        &[
            EMR_PARSER,
            "building-argument-table.emr.add-tags",
            "building-argument-table.emr.list-clusters",
        ],
    ),
    (
        "awscli.customizations.rds.register_rds_modify_split",
        &[
            "building-argument-table.rds.add-option-to-option-group",
            "building-argument-table.rds.remove-option-from-option-group",
        ],
    ),
    (
        "awscli.customizations.sso.register_sso_commands",
        &["session-initialized"],
    ),
];

/// The rows reached through root table events are the initializers the
/// pack recorded as table editors, each listed under root table events and,
/// for [`MIXED`], exactly its other patterns: the rows describe the AWS CLI
/// the fallback runs, and the pack's list of editors and the table's cannot
/// drift apart.
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
            assert!(
                listed.iter().any(|pattern| root_table_event(pattern)),
                "{}: {listed:?}",
                row.entry
            );
            let mut others: Vec<&str> = listed
                .into_iter()
                .filter(|pattern| !root_table_event(pattern))
                .collect();
            others.sort_unstable();
            let expected = MIXED
                .iter()
                .find(|(entry, _)| *entry == row.entry)
                .map_or(&[][..], |(_, patterns)| patterns);
            let mut expected = expected.to_vec();
            expected.sort_unstable();
            assert_eq!(others, expected, "{}", row.entry);
            for pattern in others {
                assert!(!row.reached.by(pattern), "{} through {pattern}", row.entry);
            }
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
    })
}

/// The command adders' tables reproduce their failures, but for the
/// tables they also edit through other patterns.
#[test]
fn the_command_adders_leave_only_their_other_tables_to_python() -> Result<()> {
    with_models(|models| {
        for (command, operation) in ADDED_TO {
            assert!(models.reproduces_failures(command, None), "{command}");
            assert!(
                models.reproduces_failures(command, Some(operation)),
                "{command} {operation}"
            );
        }
        assert!(
            !models.reproduces_failures("emr", Some("list-clusters")),
            "emr_initialize edits emr's list-clusters table"
        );
        assert!(
            !models.reproduces_failures("emr", Some("list-instances")),
            "emr_initialize's parser hook acts on every emr table"
        );
        for operation in [
            "create-distribution",
            "create-invalidation",
            "update-distribution",
        ] {
            assert!(
                !models.reproduces_failures("cloudfront", Some(operation)),
                "cloudfront.register edits {operation}'s table"
            );
        }
        assert!(
            !models.reproduces_failures("agent-toolkit", None),
            "register_agent_toolkit_commands reads its table, so is no recorded editor"
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
        with_argv(
            &["cloudfront", "list-distributions"],
            &[
                "cloudfront",
                "update-distribution",
                "--id",
                "a",
                "--if-match",
                "b",
                "--default-root-object",
                "index.html",
            ],
        ),
    ),
    (
        ".add_generate_db_auth_token",
        with_argv(
            &["rds", "describe-db-instances"],
            &[
                "rds",
                "generate-db-auth-token",
                "--hostname",
                "h",
                "--port",
                "5432",
                "--username",
                "u",
            ],
        ),
    ),
    (
        ".add_dsql_generate_db_auth_token_methods",
        with_argv(
            &["dsql", "list-clusters"],
            &["dsql", "generate-db-connect-auth-token", "--hostname", "h"],
        ),
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
        "building-command-table.agent-toolkit",
        "awscli.customizations.agenttoolkit.register_agent_toolkit_commands",
    )];
    assert!(uncovered(&other, &[], "agent-toolkit", "Op").is_some());
    for (entry, patterns) in MIXED {
        for pattern in *patterns {
            let found = [registry(pattern, entry)];
            let covered = CLI
                .iter()
                .any(|row| row.entry == *entry && row.reached.by(pattern));
            assert_eq!(
                uncovered(&found, &[], "svc", "Op").is_none(),
                covered,
                "{entry} through {pattern}"
            );
        }
    }
    let shared = [registry(
        "top-level-args-parsed",
        "awscli.customizations.cloudfront.register",
    )];
    assert_eq!(uncovered(&shared, &[], "cloudfront", "Op"), None);
}

/// A command adder's names are no operations of the source, so invoking
/// one is Python's, as a command the parse resolves to no operation
/// (`UnsupportedCommand`, as `wizard` is): each it adds, each model
/// operation it replaces, and
/// rds's modify-option-group, which it deletes; the service's other
/// operations are its operations still.
#[test]
fn a_command_adder_s_names_are_python_s() -> Result<()> {
    let replaced = [
        ("ssm", "start-session"),
        ("ecs", "execute-command"),
        ("logs", "start-live-tail"),
        ("sso", "logout"),
        ("emr", "describe-cluster"),
        ("emr", "add-instance-groups"),
    ];
    let added = [
        ("logs", "tail"),
        ("eks", "get-token"),
        ("ecs", "deploy"),
        ("cloudfront", "sign"),
        ("rds", "add-option-to-option-group"),
    ];
    with_models(|models| {
        for (command, name) in replaced.iter().chain(&added) {
            let service = models
                .service(command)
                .unwrap_or_else(|| panic!("{command}"));
            assert!(
                service.choices.iter().any(|choice| choice == name),
                "{command} {name}"
            );
            assert!(service.operation(name).is_none(), "{command} {name}");
        }
        let rds = models.service("rds").unwrap_or_else(|| panic!("rds"));
        assert!(
            !rds.choices
                .iter()
                .any(|choice| choice == "modify-option-group")
        );
        for (command, operation) in ADDED_TO {
            let service = models
                .service(command)
                .unwrap_or_else(|| panic!("{command}"));
            assert!(
                service.operation(operation).is_some(),
                "{command} {operation}"
            );
        }
    })?;
    for (command, name) in replaced.iter().chain(&added) {
        let verdict = verdict(&[command, name], "", "")?;
        assert!(
            matches!(verdict, Verdict::Python(PythonReason::UnsupportedCommand)),
            "{command} {name}: {verdict:?}"
        );
    }
    Ok(())
}
