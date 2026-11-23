//! The model source over the built payload, with the repository's fake
//! models on `AWS_DATA_PATH` where a test needs them.
#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use pyload::View;
use std::ffi::OsString;
use std::path::PathBuf;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The fallback payload the product embeds, as `cargo xtask` provides it.
fn payload() -> Result<Vec<u8>> {
    let prefix = std::env::var_os("AWS_NATIVE_FALLBACK_PREFIX")
        .ok_or("AWS_NATIVE_FALLBACK_PREFIX must name the fallback payload prefix")?;
    Ok(std::fs::read(PathBuf::from(prefix).join("awscli-v2.blob"))?)
}

fn fake_models() -> OsString {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .into_os_string()
}

/// `os.path.expanduser(os.path.expandvars(path))` over an environment with
/// a home directory and nothing else.
fn expand(path: &[u8]) -> Option<Vec<u8>> {
    let env =
        std::collections::HashMap::from([("HOME".to_owned(), OsString::from("/nonexistent"))]);
    let vars = awscli_argparse::unpack::expand_vars(path, &env)?;
    awscli_argparse::unpack::expand_user(&vars, &env)
}

/// `check` of the source over the payload, `data_path` and nothing
/// reproduced: every table an initializer edits is customized.
fn with_source<T>(
    data_path: Option<OsString>,
    check: impl FnOnce(&ModelSource<'_>) -> T,
) -> Result<T> {
    with_reproduced(data_path, &[], check)
}

/// Every registry initializer that `operations`' tables and their
/// services' command tables meet: with all of them reproduced, none of
/// those tables is customized.
fn met(data_path: Option<OsString>, operations: &[(&str, &str)]) -> Result<Vec<&'static str>> {
    with_source(data_path, |source| {
        let mut met = Vec::new();
        for (command, operation) in operations {
            let events = [format!("building-command-table.{command}")];
            met.extend(touching(source, command, operation));
            met.extend(source.patterns().registry_entries(&events));
        }
        met.into_iter()
            .map(|name| &*Box::leak(name.into_boxed_str()))
            .collect()
    })
}

fn with_reproduced<T>(
    data_path: Option<OsString>,
    reproduced: &[&'static str],
    check: impl FnOnce(&ModelSource<'_>) -> T,
) -> Result<T> {
    let bytes = payload()?;
    let view = View::new(&bytes)?;
    let source = ModelSource::from_payload(view, data_path.as_deref(), &expand, reproduced)?;
    Ok(check(&source))
}

fn names(table: &Table) -> Vec<&str> {
    table
        .arguments
        .iter()
        .map(|argument| &*argument.name)
        .collect()
}

fn argument<'t>(table: &'t Table, name: &str) -> &'t DerivedArgument {
    table
        .arguments
        .iter()
        .find(|argument| &*argument.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn roots_are_model_directories_after_the_main_table_s_edits() -> Result<()> {
    with_source(Some(fake_models()), |source| {
        for (word, root) in [
            ("sts", true),
            ("zquery", true),
            ("zjson", true),
            ("s3api", true),
            ("deploy", true),
            ("codedeploy", false),
            ("configure", false),
            ("s3", false),
            ("nosuch", false),
        ] {
            assert_eq!(source.is_root(word), root, "{word}");
        }
        let roots = source.roots().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(roots.get("s3api").map(String::as_str), Some("s3"));
        assert_eq!(roots.get("deploy").map(String::as_str), Some("codedeploy"));
    })
}

#[test]
fn a_fake_model_is_no_root_without_the_data_path() -> Result<()> {
    with_source(None, |source| assert!(!source.is_root("zquery")))
}

#[test]
fn a_service_s_table_is_its_operations_then_wait() -> Result<()> {
    with_source(None, |source| {
        let sts = source.operations("sts").unwrap_or_else(|| panic!("sts"));
        assert!(sts.contains(&"get-caller-identity"));
        assert!(!sts.contains(&"wait"), "sts has no waiters");
        let dynamodb = source
            .operations("dynamodb")
            .unwrap_or_else(|| panic!("dynamodb"));
        assert_eq!(dynamodb.last(), Some(&"wait"));
        assert_eq!(dynamodb.first(), Some(&"batch-execute-statement"));
        assert!(source.operations("nosuch").is_none());
    })
}

#[test]
fn an_operation_table_is_its_members_then_the_cross_cutting_arguments() -> Result<()> {
    with_source(None, |source| {
        let table = source
            .table("sts", "assume-role")
            .unwrap_or_else(|| panic!("table"));
        let names = names(table);
        assert_eq!(names[..2], ["role-arn", "role-session-name"]);
        assert_eq!(
            names[names.len() - 3..],
            ["cli-input-json", "cli-input-yaml", "generate-cli-skeleton"]
        );
        assert!(argument(table, "role-arn").required);
        assert_eq!(
            argument(table, "policy-arns").nargs,
            Some(awscli_argparse::Nargs::ZeroOrMore)
        );
        assert_eq!(argument(table, "duration-seconds").value_type, None);
        let skeleton = argument(table, "generate-cli-skeleton");
        assert_eq!(skeleton.constant, Some("input"));
        assert_eq!(table.overrides.len(), 3);
        assert!(!table.typed());
    })
}

#[test]
fn a_paginated_operation_hides_its_tokens_and_adds_the_page_options() -> Result<()> {
    with_source(None, |source| {
        let table = source
            .table("dynamodb", "list-tables")
            .unwrap_or_else(|| panic!("table"));
        assert!(argument(table, "exclusive-start-table-name").hidden);
        assert!(argument(table, "limit").hidden);
        for name in ["page-size", "max-items"] {
            assert_eq!(
                argument(table, name).value_type,
                Some(ValueType::Int),
                "{name}"
            );
        }
        assert_eq!(argument(table, "starting-token").value_type, None);
        assert!(table.typed());
    })
}

/// argparse's value types are parsed natively, so a typed table renders
/// its own failures where nothing customizes it.
#[test]
fn a_typed_table_renders_its_failures_natively() -> Result<()> {
    let operations = [("zjson", "list-things"), ("zjson", "measure")];
    let met = met(Some(fake_models()), &operations)?;
    with_reproduced(Some(fake_models()), &met, |source| {
        let table = source
            .table("zjson", "list-things")
            .unwrap_or_else(|| panic!("table"));
        assert!(table.typed());
        assert!(source.reproduces_failures("zjson", Some("list-things")));
        assert!(source.reproduces_failures("zjson", Some("measure")));
    })
}

#[test]
fn booleans_pair_with_their_negations_and_stay_optional() -> Result<()> {
    with_source(Some(fake_models()), |source| {
        let table = source
            .table("zquery", "delete-thing")
            .unwrap_or_else(|| panic!("table"));
        let force = argument(table, "force");
        let negation = argument(table, "no-force");
        assert_eq!(force.action, awscli_argparse::Action::StoreTrue);
        assert_eq!(negation.action, awscli_argparse::Action::StoreFalse);
        assert_eq!(negation.dest.as_deref(), Some("force"));
        assert!(
            force.default_null && !force.required,
            "required booleans parse as optional"
        );
        let get = source
            .table("zquery", "get-thing")
            .unwrap_or_else(|| panic!("get"));
        assert!(
            !argument(get, "client-token").required,
            "idempotency tokens are optional"
        );
        assert!(argument(get, "thing-id").required);
    })
}

/// A table that an initializer outside the reproduced set edits is
/// customized, and its failures are Python's; with every initializer its
/// events meet reproduced, it is not.
#[test]
fn customized_tables_leave_their_failures_to_python() -> Result<()> {
    with_source(None, |source| {
        assert!(!source.reproduces_failures("sts", Some("get-caller-identity")));
        assert!(!source.reproduces_failures("nosuch", None));
    })?;
    let met = met(None, &[("sts", "get-caller-identity")])?;
    with_reproduced(None, &met, |source| {
        assert!(source.reproduces_failures("sts", None));
        assert!(source.reproduces_failures("sts", Some("get-caller-identity")));
        assert!(!source.reproduces_failures("ec2", None));
    })
}

/// A client is S3's, as far as settings go, when its rule set takes S3's
/// built-ins: botocore fills them from the `s3` block.
#[test]
fn a_model_operation_needs_its_service_s_client() -> Result<()> {
    with_source(None, |source| {
        assert_eq!(
            source.needs("sts"),
            Needs {
                service: "sts".into(),
                reads_s3_settings: false,
            }
        );
        assert!(source.needs("s3api").reads_s3_settings);
        assert!(source.needs("s3control").reads_s3_settings);
    })
}

/// The awscli entries and botocore handlers that touch `[command,
/// operation]`, by dotted name, each once and in order.
fn touching(source: &ModelSource<'_>, command: &str, operation: &str) -> Vec<String> {
    entry_names(
        source
            .matches(command, operation)
            .unwrap_or_else(|| panic!("{command} {operation}")),
    )
}

fn entry_names(found: Vec<crate::patterns::Match>) -> Vec<String> {
    let names: std::collections::BTreeSet<String> =
        found.into_iter().map(|found| found.name).collect();
    names.into_iter().collect()
}

const SHORTHAND: &str = "awscli.argprocess.register_param_shorthand_parser";
const CLI_INPUT: &str = "awscli.customizations.cliinput.register_cli_input_args";
const DYNAMODB_FIX: &str =
    "awscli.customizations.dynamodb.paginatorfix.register_dynamodb_paginator_fix";
const ARGRENAME: &str = "awscli.customizations.argrename.register_arg_renames";

#[test]
fn sts_get_caller_identity_meets_only_cross_cutting_entries() -> Result<()> {
    with_source(None, |source| {
        let entries = touching(source, "sts", "get-caller-identity");
        for wanted in [
            SHORTHAND,
            CLI_INPUT,
            "awscli.paramfile.register_init_uri_param_handler",
        ] {
            assert!(entries.iter().any(|entry| entry == wanted), "{wanted}");
        }
        for unwanted in [
            DYNAMODB_FIX,
            ARGRENAME,
            "awscli.customizations.removals.register_removals",
        ] {
            assert!(!entries.iter().any(|entry| entry == unwanted), "{unwanted}");
        }
    })
}

#[test]
fn the_matcher_adds_each_service_s_own_entries() -> Result<()> {
    with_source(None, |source| {
        for (command, operation, wanted) in [
            ("dynamodb", "list-tables", DYNAMODB_FIX),
            ("ec2", "describe-instances", ARGRENAME),
            (
                "ec2",
                "describe-instances",
                "awscli.customizations.toplevelbool.register_bool_params",
            ),
            (
                "ec2",
                "describe-instances",
                "awscli.customizations.removals.register_removals",
            ),
            (
                "iam",
                "create-virtual-mfa-device",
                "awscli.customizations.iamvirtmfa.IAMVMFAWrapper",
            ),
            (
                "s3api",
                "put-object",
                "awscli.customizations.s3errormsg.register_s3_error_msg",
            ),
            (
                "s3api",
                "put-object",
                "botocore.handlers.convert_body_to_file_like_object",
            ),
        ] {
            let entries = touching(source, command, operation);
            assert!(
                entries.iter().any(|entry| entry == wanted),
                "{command} {operation}: {wanted}"
            );
        }
        let other = touching(source, "iam", "list-users");
        assert!(!other.iter().any(|entry| entry.contains("iamvirtmfa")));
    })
}

/// The fake models' names match no Python customization: every operation
/// of every fake service is touched by the same, cross-cutting entries.
#[test]
fn the_fake_models_meet_only_cross_cutting_customizations() -> Result<()> {
    with_source(Some(fake_models()), |source| {
        let mut seen: Option<(String, Vec<String>)> = None;
        for service in ["zquery", "zjson", "zjson11", "zglobal"] {
            let operations = source.operations(service).unwrap_or_default();
            assert!(!operations.is_empty(), "{service} has no operations");
            for operation in operations.iter() {
                let touched = source
                    .matches(service, operation)
                    .map(entry_names)
                    .unwrap_or_default();
                let at = format!("{service} {operation}");
                match &seen {
                    None => seen = Some((at, touched)),
                    Some((first, expected)) => assert_eq!(&touched, expected, "{at} vs {first}"),
                }
            }
        }
    })
}

fn client_handlers(source: &ModelSource<'_>, command: &str, operation: &str) -> Vec<String> {
    source
        .client_handlers(command, operation)
        .unwrap_or_else(|| panic!("{command} {operation}"))
}

fn generic(source: &ModelSource<'_>) -> Vec<String> {
    let mut rows: Vec<String> = source
        .patterns()
        .client
        .generic
        .iter()
        .map(|row| row.handler.to_string())
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

/// The pack observed every client's creation: a service with an
/// endpoint-discovery operation registers the discovery block, s3control
/// its ARN handler, and a service botocore does nothing special for only
/// the rows every client registers.
#[test]
fn client_creation_rows_are_the_service_s_and_the_generic_ones() -> Result<()> {
    with_source(None, |source| {
        let generic = generic(source);
        assert!(!generic.is_empty());
        for (command, operation, wanted) in [
            (
                "dynamodb",
                "list-tables",
                "botocore.discovery.block_endpoint_discovery_required_operations",
            ),
            (
                "s3control",
                "list-access-points",
                "botocore.utils.S3ControlArnParamHandlerv2.handle_arn",
            ),
        ] {
            let rows = client_handlers(source, command, operation);
            assert!(rows.iter().any(|row| row == wanted), "{command}: {rows:?}");
            assert!(generic.iter().all(|row| rows.contains(row)), "{command}");
        }
        assert_eq!(
            client_handlers(source, "sts", "get-caller-identity"),
            generic
        );
        assert_eq!(client_handlers(source, "sns", "list-topics"), generic);
        let touched = touching(source, "sts", "get-caller-identity");
        assert!(generic.iter().all(|row| touched.contains(row)));
    })
}

/// A user model's client meets the generic rows alone: what the pack saw
/// for a service is its packed model's.
#[test]
fn a_user_model_s_client_meets_the_generic_rows() -> Result<()> {
    with_source(Some(fake_models()), |source| {
        assert_eq!(client_handlers(source, "zjson", "measure"), generic(source));
    })
}
