use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn failure(values: &[&str]) -> Option<String> {
    parse(&args(values)).err().map(|error| error.to_string())
}

#[test]
fn no_option_leaves_every_choice_to_the_defaults() -> Result<(), TaskError> {
    assert_eq!(parse(&[])?, Options::default());
    Ok(())
}

#[test]
fn a_side_is_a_binary_and_a_route() -> Result<(), TaskError> {
    let options = parse(&args(&[
        "--a",
        "candidate:python",
        "--b",
        "old/a:b/aws:unset",
    ]))?;
    assert_eq!(
        options.a,
        Some(SideSpec {
            binary: Binary::Candidate,
            route: Route::Python
        })
    );
    assert_eq!(
        options.b,
        Some(SideSpec {
            binary: Binary::Path(PathBuf::from("old/a:b/aws")),
            route: Route::Unset
        })
    );
    for text in ["candidate", "candidate:", ":native", "aws:fallback"] {
        let error = failure(&["--a", text]).unwrap_or_default();
        assert!(
            error.starts_with("a side is candidate:ROUTE"),
            "{text}: {error}"
        );
    }
    Ok(())
}

#[test]
fn the_other_options_take_their_values() -> Result<(), TaskError> {
    let options = parse(&args(&[
        "--aws-binary",
        "b/aws",
        "--corpus",
        "x.jsonl",
        "--corpus",
        "y.jsonl",
        "--filter",
        "seed",
        "--known",
        "k.json",
        "--out",
        "o",
    ]))?;
    assert_eq!(
        options,
        Options {
            aws_binary: args(&["--aws-binary", "b/aws"]),
            corpus: vec![PathBuf::from("x.jsonl"), PathBuf::from("y.jsonl")],
            filter: Some("seed".to_owned()),
            known: Some(PathBuf::from("k.json")),
            out: Some(PathBuf::from("o")),
            ..Options::default()
        }
    );
    Ok(())
}

#[test]
fn the_self_test_takes_only_the_candidate() -> Result<(), TaskError> {
    let options = parse(&args(&["--self-test", "--aws-binary", "b/aws"]))?;
    assert!(options.self_test);
    assert_eq!(
        failure(&["--self-test", "--filter", "x"]).as_deref(),
        Some("--self-test takes only --aws-binary")
    );
    Ok(())
}

#[test]
fn unknown_options_and_missing_values_fail() {
    assert_eq!(
        failure(&["--check"]).as_deref(),
        Some("unknown diff option: --check")
    );
    assert_eq!(
        failure(&["--out"]).as_deref(),
        Some("--out requires a value")
    );
}
