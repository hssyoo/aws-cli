use super::*;
use format::Query as _;

fn value(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

/// What the native route evaluates compiles, and searches as `jmespath`
/// does.
#[test]
fn an_admitted_expression_searches_as_jmespath_does() -> Result<(), PythonReason> {
    let response = value(
        r#"{"Vpcs": [{"Id": "b", "On": true, "N": 3}, {"Id": "a", "N": 1}], "Meta": {"N": 2}}"#,
    );
    for (text, found) in [
        ("Meta.N", "2"),
        ("Missing", "null"),
        ("Vpcs[].Id", r#"["b", "a"]"#),
        ("Vpcs[?On].Id", r#"["b"]"#),
        ("Vpcs[?Id == 'a'].N", "[1]"),
        ("Vpcs[?N > `1`].Id", r#"["b"]"#),
        ("Vpcs[:1].Id", r#"["b"]"#),
        ("Vpcs[::-1].Id", r#"["a", "b"]"#),
        ("[Meta.N, Vpcs[0].Id]", r#"[2, "b"]"#),
        ("{n: Meta.N}", r#"{"n": 2}"#),
        ("Vpcs[*].Id | [0]", r#""b""#),
        ("length(Vpcs)", "2"),
        ("sort_by(Vpcs, &Id)[].Id", r#"["a", "b"]"#),
        ("max_by(Vpcs, &N).Id", r#""b""#),
        ("min_by(Vpcs, &N).Id", r#""a""#),
        ("map(&Id, Vpcs)", r#"["b", "a"]"#),
        ("join(',', sort(Vpcs[].Id))", r#""a,b""#),
        ("to_string(Meta)", r#""{\"N\":2}""#),
        ("sum(Vpcs[].N)", "4"),
        ("`{}`", "{}"),
    ] {
        let query = Query::compile(text)?;
        assert_eq!(query.search(&response), Ok(value(found)), "{text}");
    }
    Ok(())
}

/// A search that raises is the exception's `str()`, which the general
/// handler prints.
#[test]
fn a_raising_search_is_python_s_text() -> Result<(), PythonReason> {
    let response = value(r#"{"Meta": {"N": 2}, "Ids": ["a", 1]}"#);
    for (text, message) in [
        (
            "length(Meta.N)",
            "In function length(), invalid type for value: 2, expected one of: \
             ['string', 'array', 'object'], received: \"number\"",
        ),
        (
            "Meta.N < 'a'",
            "'<' not supported between instances of 'int' and 'str'",
        ),
    ] {
        let found = Query::compile(text)?.search(&response);
        match found {
            Err(error) => assert_eq!(error.message, message, "{text}"),
            Ok(found) => assert_eq!(Ok(found), Err(()), "{text}"),
        }
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
        "`[1",
        // Python raises UnknownFunctionError only once it calls one.
        "nosuch(@)",
        // Python reads a list of pairs where the engine refuses.
        "merge(`{}`)",
        "merge(@, a)",
        // An expression reference Python would hold as a value, or print
        // with its address.
        "&Id",
        "[&Id]",
        "{a: &Id}",
        "not_null(&Id)",
        "length(&Id)",
        "to_string(&Id)",
        "sort_by(&Id, @)",
        "map(@, &Id)",
        "sort_by(@, (&Id))",
        "\"sort_by\"(@, &Id)",
        "max_by(@, &length(&x))",
        "sort_by(@, &a) || &b",
        "Vpcs[?&Id]",
    ] {
        assert_eq!(
            Query::compile(text).map(|_| ()),
            Err(PythonReason::PythonOnlyGlobal),
            "{text:?}"
        );
    }
}

/// An expression reference is admitted where the function evaluates it,
/// past literals, raw strings, quoted names and `&&` that hold an `&` or
/// a `,` of their own.
#[test]
fn an_expression_reference_is_admitted_as_its_function_s_argument() {
    for text in [
        "sort_by(@, &Id)",
        "sort_by( @ , & Id )",
        "min_by(Vpcs, &N)",
        "max_by(Vpcs, &to_number(N))",
        "map(&Id, Vpcs)",
        "map(&sort_by(@, &a), @)",
        "sort_by(Vpcs, &[N, Id][0])",
        "sort_by(@, &Id)[?a && b]",
        "[sort_by(@, &a), map(&b, @)]",
        "sort_by(@, &'x,&y')",
        "sort_by(@, &`\"&\"`)",
        "sort_by(@, &\"a,&\")",
        "sort_by(join(',', @), &a)",
    ] {
        assert!(Query::compile(text).is_ok(), "{text:?}");
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
    for query in ["Account", "length(Arn)", "sort_by([Arn], &@)"] {
        let argv = ["--query", query, "sts", "get-caller-identity"];
        let verdict = crate::testing::route(&argv, &[], &files)?;
        assert!(
            matches!(verdict, cli_router::Verdict::Run(_)),
            "{query}: {verdict:?}"
        );
    }
    for argv in [
        &["--query", "merge(@)", "sts", "get-caller-identity"][..],
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
