use super::*;
use format::Query as _;

fn value(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

/// What the native route evaluates compiles, and searches as `jmespath`
/// does.
#[test]
fn an_admitted_expression_searches_as_jmespath_does() -> Result<(), PythonReason> {
    let response = value(r#"{"Vpcs": [{"Id": "a", "On": true}, {"Id": "b"}], "Meta": {"N": 2}}"#);
    for (text, found) in [
        ("Meta.N", "2"),
        ("Missing", "null"),
        ("Vpcs[].Id", r#"["a", "b"]"#),
        ("Vpcs[?On].Id", r#"["a"]"#),
        ("Vpcs[1]", r#"{"Id": "b"}"#),
        ("[Meta.N, Vpcs[0].Id]", r#"[2, "a"]"#),
        ("{n: Meta.N}", r#"{"n": 2}"#),
        ("Vpcs[*].Id | [0]", r#""a""#),
        ("[Vpcs[].Id, [`1`]][]", r#"["a", "b", 1]"#),
        ("`{}`", "{}"),
        (
            "@",
            r#"{"Vpcs": [{"Id": "a", "On": true}, {"Id": "b"}], "Meta": {"N": 2}}"#,
        ),
    ] {
        let query = Query::compile(text)?;
        assert_eq!(query.search(&response), Ok(value(found)), "{text}");
    }
    Ok(())
}

/// An expression that does not compile, or that the native route does
/// not evaluate as Python does, is Python's before any request.
#[test]
fn the_rest_is_python_s() {
    for text in [
        "",
        "[",
        "a ^ b",
        "foo(",
        "length(Vpcs)",
        "sort_by(Vpcs, &Id)",
        "merge(`{}`)",
        "nosuch(@)",
        "Vpcs[?Id == 'a']",
        "Vpcs[:1]",
        "&Id",
        "`[1",
    ] {
        assert_eq!(
            Query::compile(text).map(|_| ()),
            Err(PythonReason::PythonOnlyGlobal),
            "{text:?}"
        );
    }
}

/// Python's parser and interpreter recurse with each level; a deep
/// expression is Python's, whatever it is, and one at the limit is not.
#[test]
fn nesting_past_the_limit_is_python_s() {
    let deep = |levels: usize| format!("{}a{}", "(".repeat(levels), ")".repeat(levels));
    assert!(Query::compile(&deep(NESTING_LIMIT)).is_ok());
    assert!(Query::compile(&deep(NESTING_LIMIT + 1)).is_err());
    let over = NESTING_LIMIT + 1;
    let literal = format!("`{}1{}`", "[".repeat(over), "]".repeat(over));
    assert!(Query::compile(&literal).is_err());
    let negations = format!("{}a", "!".repeat(over));
    assert!(Query::compile(&negations).is_err());
}

/// The binding compiles `--query`: an expression it evaluates binds, and
/// one it does not routes the command to Python, ahead of the paging
/// handler's failure, which `resolve_types` also precedes.
#[test]
fn the_binding_compiles_the_query() -> crate::testing::Result<()> {
    let files = crate::testing::files("", "");
    let argv = ["--query", "Account", "sts", "get-caller-identity"];
    let verdict = crate::testing::route(&argv, &[], &files)?;
    assert!(
        matches!(verdict, cli_router::Verdict::Run(_)),
        "{verdict:?}"
    );
    for argv in [
        &["--query", "length(Arn)", "sts", "get-caller-identity"][..],
        &["--query", "[", "sts", "get-caller-identity"],
        &[
            "ec2",
            "describe-vpcs",
            "--query",
            "[",
            "--no-paginate",
            "--max-items",
            "1",
        ],
    ] {
        let verdict = crate::testing::route(argv, &[], &files)?;
        assert!(
            matches!(
                verdict,
                cli_router::Verdict::Python(PythonReason::PythonOnlyGlobal)
            ),
            "{argv:?}: {verdict:?}"
        );
    }
    Ok(())
}
