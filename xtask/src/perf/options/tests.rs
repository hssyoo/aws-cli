use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn error(values: &[&str]) -> Option<String> {
    parse(&args(values)).err().map(|error| error.to_string())
}

#[test]
fn no_options_report_on_every_case_with_the_default_binary() -> Result<(), TaskError> {
    let options = parse(&[])?;
    assert_eq!(options, Options::default());
    assert!(options.is_complete());
    assert!(options.binary_args().is_empty());
    Ok(())
}

#[test]
fn every_option_is_read() -> Result<(), TaskError> {
    let options = parse(&args(&[
        "--check",
        "--full",
        "--aws-binary",
        "before/aws",
        "--commit",
        "abc",
        "--case",
        "query-single",
        "--case",
        "s3-ls",
        "--route",
        "native",
        "--runs",
        "20",
        "--warmup",
        "0",
        "--out",
        "before.json",
    ]))?;
    assert_eq!(
        options,
        Options {
            mode: Mode::Check,
            tier: Tier::Full,
            binary: Some("before/aws".into()),
            commit: Some("abc".into()),
            cases: vec!["query-single".into(), "s3-ls".into()],
            routes: vec!["native".into()],
            runs: Some(20),
            warmup: Some(0),
            out: Some(PathBuf::from("before.json")),
        }
    );
    assert_eq!(options.binary_args(), ["--aws-binary", "before/aws"]);
    assert!(!options.is_complete());
    Ok(())
}

#[test]
fn a_selection_or_another_run_count_is_not_complete() -> Result<(), TaskError> {
    for selection in [
        &["--case", "s3-ls"][..],
        &["--route", "python"],
        &["--runs", "3"],
        &["--warmup", "1"],
    ] {
        assert!(!parse(&args(selection))?.is_complete(), "{selection:?}");
    }
    assert!(parse(&args(&["--write-baseline", "--out", "x.json"]))?.is_complete());
    Ok(())
}

#[test]
fn bad_options_are_refused_by_name() {
    assert_eq!(
        error(&["--fast"]),
        Some("unknown perf option: --fast".into())
    );
    assert_eq!(error(&["--runs"]), Some("--runs requires a value".into()));
    assert_eq!(
        error(&["--runs", "0"]),
        Some("--runs takes a whole number of at least 1".into())
    );
    assert_eq!(
        error(&["--warmup", "-1"]),
        Some("--warmup takes a whole number of at least 0".into())
    );
    assert_eq!(
        error(&["--route", "auto"]),
        Some("--route is native or python, not auto".into())
    );
    assert_eq!(
        error(&["--check", "--write-baseline"]),
        Some("--check and --write-baseline exclude each other".into())
    );
}

#[test]
fn the_check_is_quick_unless_full() -> Result<(), TaskError> {
    assert_eq!(parse(&args(&["--check"]))?.tier, Tier::Quick);
    assert_eq!(parse(&args(&["--check", "--full"]))?.tier, Tier::Full);
    Ok(())
}
