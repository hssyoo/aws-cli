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

fn with_source<T>(
    data_path: Option<OsString>,
    check: impl FnOnce(&ModelSource<'_>) -> T,
) -> Result<T> {
    let bytes = payload()?;
    let view = View::new(&bytes)?;
    let source = ModelSource::from_payload(view, data_path.as_deref(), &expand)?;
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
        assert!(!table.customized && !table.typed());
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
    with_source(Some(fake_models()), |source| {
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

#[test]
fn customized_tables_leave_their_failures_to_python() -> Result<()> {
    with_source(None, |source| {
        assert!(source.reproduces_failures("sts", None));
        assert!(source.reproduces_failures("sts", Some("get-caller-identity")));
        assert!(
            !source.reproduces_failures("ec2", None),
            "removals edit ec2's table"
        );
        assert!(!source.reproduces_failures("nosuch", None));
    })
}

#[test]
fn a_model_operation_binds_nothing_yet_and_needs_its_service_s_client() -> Result<()> {
    with_source(None, |source| {
        assert_eq!(
            source.needs("sts"),
            Needs::Client {
                service: "sts".into()
            }
        );
        assert!(matches!(
            source.bind(&ParseResult::default()),
            Err(Admission::Python(PythonReason::UnportedCustomization))
        ));
    })
}

fn touching(source: &ModelSource<'_>, command: &str, operation: &str) -> Vec<String> {
    source
        .touching(command, operation)
        .unwrap_or_else(|| panic!("{command} {operation}"))
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
