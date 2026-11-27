#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::ServiceSource;
use crate::testing::{Result, parsed, with_models};
use cli_core::cli_error::Category;
use cli_router::Verdict;

fn bound(source: &ServiceSource<'_>, argv: &[&str]) -> std::result::Result<Binding, PythonReason> {
    let result = parsed(source, argv);
    let table = source
        .models()
        .table(argv[0], argv[1])
        .unwrap_or_else(|| panic!("no table"));
    bind(table, &result)
}

fn call(source: &ServiceSource<'_>, argv: &[&str]) -> Call {
    match bound(source, argv) {
        Ok(Binding::Call(call)) => call,
        other => panic!("{argv:?}: {other:?}"),
    }
}

fn input(source: &ServiceSource<'_>, argv: &[&str]) -> Value {
    call(source, argv).input
}

fn raised(source: &ServiceSource<'_>, argv: &[&str]) -> String {
    match bound(source, argv) {
        Ok(Binding::Raised(failure)) => {
            assert_eq!(failure.category, Category::ParamValidation);
            String::from_utf8_lossy(&failure.message).into_owned()
        }
        other => panic!("{argv:?}: {other:?}"),
    }
}

fn value(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or_else(|error| panic!("{error}"))
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_map()
        .map(|map| map.keys().filter_map(model::value::Key::as_str).collect())
        .unwrap_or_else(|| panic!("{value:?}"))
}

/// The request holds the supplied members in the table's (model) order,
/// whatever argv's, an integer at any magnitude, a boolean's twin as its
/// last spelling, and nothing for what is unset.
#[test]
fn members_bind_in_model_order_as_add_to_params_stores_them() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let value = input(
            &source,
            &[
                "zquery",
                "get-thing",
                "--count",
                "99999999999999999999999",
                "--thing-id",
                "a",
            ],
        );
        assert_eq!(keys(&value), ["ThingId", "Count"]);
        assert_eq!(
            value.get("Count"),
            Some(&Value::BigInteger("99999999999999999999999".into()))
        );
        let value = input(
            &source,
            &[
                "zquery",
                "get-thing",
                "--thing-id",
                "a",
                "--count",
                " 0_7 ",
                "--enabled",
                "--no-enabled",
            ],
        );
        assert_eq!(value.get("Count"), Some(&Value::Integer(7)));
        assert_eq!(value.get("Enabled"), Some(&Value::Bool(false)));
        assert_eq!(keys(&value), ["ThingId", "Count", "Enabled"]);
        let value = input(
            &source,
            &["zjson", "measure", "--unit", "u", "--total", "5"],
        );
        assert_eq!(keys(&value), ["Unit", "Total"]);
    })
}

/// The page options go to `PaginationConfig` as argparse typed them, not
/// to the request, and paging is on with nothing given.
#[test]
fn the_page_options_bind_into_the_pagination_config() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let plain = call(&source, &["zjson", "list-things"]);
        assert_eq!(plain.input, value("{}"));
        assert_eq!(plain.pagination, Some(PaginationConfig::default()));
        assert!(!plain.warned);
        let paged = call(
            &source,
            &[
                "zjson",
                "list-things",
                "--max-items",
                " 1_0 ",
                "--page-size",
                "99999999999999999999",
                "--starting-token",
                "token",
            ],
        );
        assert_eq!(paged.input, value("{}"));
        assert_eq!(
            paged.pagination,
            Some(PaginationConfig {
                max_items: Some(Value::Integer(10)),
                page_size: Some(Value::BigInteger("99999999999999999999".into())),
                starting_token: Some("token".into()),
            })
        );
    })
}

/// A string-typed limit key types the page options as strings, which
/// `add_to_params` keeps; its `int()` of `--max-items` warns for a value
/// that is not positive and raises, on the Python route, for no integer.
#[test]
fn a_string_limit_key_keeps_the_page_options_as_text() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let paged = call(
            &source,
            &[
                "apigatewayv2",
                "get-apis",
                "--max-items",
                " 0 ",
                "--page-size",
                "2",
            ],
        );
        assert!(paged.warned);
        assert_eq!(
            paged.pagination,
            Some(PaginationConfig {
                max_items: Some(Value::String(" 0 ".into())),
                page_size: Some(Value::String("2".into())),
                starting_token: None,
            })
        );
        assert_eq!(
            bound(&source, &["apigatewayv2", "get-apis", "--max-items", "abc"]),
            Err(PythonReason::UnsupportedValue)
        );
    })
}

/// `PageArgument.add_to_params` warns for a `--max-items` that is not
/// positive, and pages anyway.
#[test]
fn a_max_items_that_is_not_positive_warns() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        for (max_items, warns) in [("0", true), ("-2", true), ("1", false)] {
            let paged = call(&source, &["zjson", "list-things", "--max-items", max_items]);
            assert_eq!(paged.warned, warns, "{max_items}");
            assert!(paged.pagination.is_some());
        }
    })
}

/// `check_should_enable_pagination`: an input token or the limit key given
/// by its own argument is manual paging, a single call with that member.
#[test]
fn a_manual_token_or_limit_turns_paging_off() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let manual = call(&source, &["zjson", "list-things", "--next-token", "x"]);
        assert_eq!(manual.pagination, None);
        assert_eq!(manual.input, value(r#"{"NextToken": "x"}"#));
        let limited = call(&source, &["zjson", "list-things", "--max-results", "2"]);
        assert_eq!(limited.pagination, None);
        assert_eq!(limited.input, value(r#"{"MaxResults": 2}"#));
        let off = call(&source, &["zjson", "list-things", "--no-paginate"]);
        assert_eq!((off.pagination, off.input), (None, value("{}")));
    })
}

/// `ensure_paging_params_not_set`: with paging off, any page option given a
/// true value raises, named in its fixed order, whether `--no-paginate` or
/// a manual token turned it off.
#[test]
fn page_options_with_paging_off_raise() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let all = [
            "zjson",
            "list-things",
            "--max-items",
            "3",
            "--no-paginate",
            "--page-size",
            "2",
            "--starting-token",
            "x",
        ];
        assert_eq!(
            raised(&source, &all),
            "Cannot specify --no-paginate along with pagination arguments: \
             --starting-token, --page-size, --max-items"
        );
        assert_eq!(
            raised(
                &source,
                &[
                    "zjson",
                    "list-things",
                    "--next-token",
                    "x",
                    "--max-items",
                    "5"
                ]
            ),
            "Cannot specify --no-paginate along with pagination arguments: --max-items"
        );
    })
}

/// A page option whose value is false as parsed does not raise; it stays in
/// the request under `PaginationConfig`, which validation then rejects, as
/// botocore's does for the single call.
#[test]
fn a_false_page_option_with_paging_off_stays_in_the_request() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let zero = call(
            &source,
            &["zjson", "list-things", "--no-paginate", "--max-items", "0"],
        );
        assert!(zero.warned);
        assert_eq!(zero.pagination, None);
        assert_eq!(
            zero.input,
            value(r#"{"PaginationConfig": {"MaxItems": 0}}"#)
        );
        let empty = call(
            &source,
            &["zjson", "list-things", "--no-paginate", "--starting-token="],
        );
        assert_eq!(
            empty.input,
            value(r#"{"PaginationConfig": {"StartingToken": ""}}"#)
        );
    })
}

/// A member a page option shadowed binds again when paging is off: acm's
/// `MaxItems` limit key under `--no-paginate`, and ssm-quicksetup's
/// `StartingToken`, an input token whose name is not normalized, so giving
/// it turns paging off. With paging on, the option binds as a page option.
#[test]
fn a_shadowed_member_binds_when_paging_is_off() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let paged = call(&source, &["acm", "list-certificates", "--max-items", "5"]);
        assert_eq!(paged.input, value("{}"));
        let max_items = paged.pagination.and_then(|pagination| pagination.max_items);
        assert_eq!(max_items, Some(Value::Integer(5)));
        let off = call(
            &source,
            &[
                "acm",
                "list-certificates",
                "--no-paginate",
                "--max-items",
                "5",
            ],
        );
        assert_eq!(
            (off.input, off.pagination),
            (value(r#"{"MaxItems": 5}"#), None)
        );
        let manual = call(
            &source,
            &[
                "ssm-quicksetup",
                "list-configurations",
                "--starting-token",
                "x",
                "--max-items",
                "3",
            ],
        );
        assert_eq!(manual.pagination, None);
        assert_eq!(
            manual.input,
            value(r#"{"MaxItems": 3, "StartingToken": "x"}"#)
        );
    })
}

/// The limit key `PageSize` is no normalized name: giving `--page-size`,
/// its shadowing option, is manual paging, a single call with the member.
#[test]
fn a_limit_key_named_page_size_is_manual_paging() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let manual = call(
            &source,
            &["elbv2", "describe-load-balancers", "--page-size", "5"],
        );
        assert_eq!(manual.pagination, None);
        assert_eq!(manual.input, value(r#"{"PageSize": 5}"#));
    })
}

/// An unpack failure is Python's when a paging handler raises or warns
/// before `_build_call_parameters` reaches it, under its own reason:
/// `ensure_paging_params_not_set` on `operation-args-parsed`, before every
/// argument, or `--max-items`' `add_to_params` when paging stays on, its
/// `int()` is not positive or raises, and it comes before the failing
/// argument in the table. Otherwise the failure is the native one.
#[test]
fn a_paging_handler_ahead_of_an_unpack_failure_has_its_reason() -> Result<()> {
    let route = |argv: &[&str]| crate::testing::route(argv, &[], &crate::testing::files("", ""));
    let family = ["zquery", "describe-defaults", "--family", "file://missing"];
    let with = |extra: &[&'static str]| [&family[..], extra].concat();
    let raised = route(&with(&["--no-paginate", "--page-size", "1"]))?;
    assert!(
        matches!(raised, Verdict::Python(PythonReason::PagingHandler)),
        "{raised:?}"
    );
    for extra in [&["--max-items", "1"][..], &["--max-items", "0"], &[]] {
        let verdict = route(&with(extra))?;
        assert!(
            matches!(&verdict, Verdict::Failure(failure) if failure.kind == "paramfile_load"),
            "{extra:?}: {verdict:?}"
        );
    }
    Ok(())
}

/// `--max-items`' warning, or its `int()` raising, precedes a failure only
/// at an argument after it in the table: `--generate-cli-skeleton` comes
/// after the page options, every member before them.
#[test]
fn max_items_precedes_only_a_later_argument_s_failure() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let table = source
            .models()
            .table("zjson", "list-things")
            .unwrap_or_else(|| panic!("no table"));
        let precedes = |max_items: &str, failed: &str| {
            let mut result = parsed(&source, &["zjson", "list-things", "--max-items", max_items]);
            result.stopped_dest = Some(failed.to_owned());
            precedes_unpack_failure(table, &result)
        };
        assert!(precedes("0", "generate_cli_skeleton"));
        assert!(precedes("-3", "generate_cli_skeleton"));
        assert!(!precedes("1", "generate_cli_skeleton"));
        assert!(!precedes("0", "max_results"));
    })
}

const MODIFY: [&str; 4] = ["ec2", "modify-instance-attribute", "--instance-id", "i-1"];

fn modify(extra: &[&'static str]) -> Vec<&'static str> {
    [&MODIFY[..], extra].concat()
}

/// toplevelbool's pair: `--flag` alone binds `{"Value": true}`, `--no-flag`
/// `{"Value": false}`, and neither nothing.
#[test]
fn a_pulled_up_pair_binds_its_value_structure() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let flag =
            |extra: &[&'static str]| input(&source, &modify(extra)).get("EbsOptimized").cloned();
        assert_eq!(
            flag(&["--ebs-optimized"]),
            Some(value(r#"{"Value": true}"#))
        );
        assert_eq!(
            flag(&["--no-ebs-optimized"]),
            Some(value(r#"{"Value": false}"#))
        );
        assert_eq!(flag(&[]), None);
        let both = input(
            &source,
            &modify(&["--no-source-dest-check", "--ebs-optimized"]),
        );
        assert_eq!(
            keys(&both),
            ["InstanceId", "EbsOptimized", "SourceDestCheck"]
        );
    })
}

/// Both halves of a pair raise `validate_boolean_mutex_groups`' error,
/// ahead of the paging check, whichever order argv gives them in.
#[test]
fn both_halves_of_a_pair_raise() -> Result<()> {
    with_models(|models| {
        let source = crate::testing::source(models, &[]);
        let expected = concat!(
            r#"Cannot specify both the "--ebs-optimized" option and "#,
            r#"the "--no-ebs-optimized" option."#
        );
        for extra in [
            &["--ebs-optimized", "--no-ebs-optimized"][..],
            &["--no-ebs-optimized", "--ebs-optimized"],
        ] {
            assert_eq!(raised(&source, &modify(extra)), expected, "{extra:?}");
        }
        let first = raised(
            &source,
            &modify(&[
                "--no-source-dest-check",
                "--no-ebs-optimized",
                "--source-dest-check",
                "--ebs-optimized",
            ]),
        );
        assert!(first.contains("\"--source-dest-check\""), "{first}");
    })
}

/// `--flag VALUE` is unpacked as the structure, which only Python does;
/// the mutex check runs before an unpack failure, which is then Python's.
#[test]
fn a_pair_s_value_and_its_conflict_route_to_python() -> Result<()> {
    let route = |argv: &[&str]| crate::testing::route(argv, &[], &crate::testing::files("", ""));
    let valued = route(&modify(&["--ebs-optimized", "Value=true"]))?;
    assert!(
        matches!(valued, Verdict::Python(PythonReason::UnsupportedOption)),
        "{valued:?}"
    );
    let missing = ["--attribute", "file://missing"];
    let conflict = route(&modify(
        &[&missing[..], &["--ebs-optimized", "--no-ebs-optimized"]].concat(),
    ))?;
    assert!(
        matches!(conflict, Verdict::Python(PythonReason::PagingHandler)),
        "{conflict:?}"
    );
    let alone = route(&modify(&missing))?;
    assert!(
        matches!(&alone, Verdict::Failure(failure) if failure.kind == "paramfile_load"),
        "{alone:?}"
    );
    Ok(())
}
