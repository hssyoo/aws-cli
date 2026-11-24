use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn error(values: &[&str]) -> Option<String> {
    parse(&args(values)).err().map(|error| error.to_string())
}

#[test]
fn no_options_report_on_the_quick_tier_with_the_default_binary() -> Result<(), TaskError> {
    let options = parse(&[])?;
    assert_eq!(options, Options::default());
    assert_eq!((options.mode, options.tier), (Mode::Report, Tier::Quick));
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
        "after/aws",
        "--commit",
        "def",
        "--base-binary",
        "before/aws",
        "--base-commit",
        "abc",
        "--case",
        "query-single",
        "--route",
        "native",
        "--runs",
        "20",
        "--warmup",
        "1",
        "--out",
        "ab.json",
    ]))?;
    assert_eq!(
        options,
        Options {
            mode: Mode::Check,
            tier: Tier::Full,
            binary: Some("after/aws".into()),
            commit: Some("def".into()),
            base_binary: Some("before/aws".into()),
            base_commit: Some("abc".into()),
            cases: vec!["query-single".into()],
            routes: vec!["native".into()],
            runs: Some(20),
            warmup: Some(1),
            out: Some(PathBuf::from("ab.json")),
            ..Options::default()
        }
    );
    assert_eq!(options.binary_args(), ["--aws-binary", "after/aws"]);
    assert!(!options.is_complete());
    Ok(())
}

#[test]
fn each_mode_has_its_flag_and_they_exclude_each_other() -> Result<(), TaskError> {
    for (flag, mode) in [
        ("--check", Mode::Check),
        ("--check-baseline", Mode::CheckBaseline),
        ("--write-baseline", Mode::WriteBaseline),
        ("--calibrate", Mode::Calibrate),
    ] {
        assert_eq!(parse(&args(&[flag]))?.mode, mode);
    }
    assert_eq!(
        parse(&args(&["--calibrate", "--repeats", "7", "--full"]))?.repeats,
        Some(7)
    );
    assert_eq!(
        error(&["--check", "--calibrate"]),
        Some(
            "--check, --check-baseline, --write-baseline and --calibrate exclude each other".into()
        )
    );
    Ok(())
}

#[test]
fn a_base_is_chosen_only_for_a_check_and_only_one_way() -> Result<(), TaskError> {
    assert_eq!(
        parse(&args(&["--check", "--base", "abc"]))?.base,
        Some("abc".into())
    );
    assert_eq!(
        parse(&args(&["--check", "--mainline", "main"]))?.mainline,
        Some("main".into())
    );
    let refusals = [
        (
            &["--base", "abc"][..],
            "--base, --base-binary and --mainline choose the base of --check",
        ),
        (
            &["--check", "--base-binary", "a", "--base", "b"],
            "--base-binary is the base itself: drop --base and --mainline",
        ),
        (
            &["--check", "--base-commit", "abc"],
            "--base-commit names the commit of a --base-binary",
        ),
        (
            &["--repeats", "3"],
            "--repeats sets the repeats of --calibrate",
        ),
        (
            &["--check", "--warmup", "0"],
            "a judged run needs a warm-up round: the first run of a new binary is slow",
        ),
    ];
    for (values, message) in refusals {
        assert_eq!(error(values), Some(message.to_owned()), "{values:?}");
    }
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
        error(&["--calibrate", "--repeats", "1"]),
        Some("--repeats takes a whole number of at least 2".into())
    );
    assert_eq!(
        error(&["--route", "auto"]),
        Some("--route is native or python, not auto".into())
    );
}
