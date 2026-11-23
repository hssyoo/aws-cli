use super::*;

#[test]
fn options_name_services_and_a_filter() -> Result<(), TaskError> {
    let args: Vec<String> = [
        "--service",
        "zquery",
        "--service",
        "sts",
        "--filter",
        "error",
    ]
    .map(str::to_owned)
    .into();
    let expected = Options {
        services: vec!["zquery".into(), "sts".into()],
        filter: Some("error".into()),
    };
    assert_eq!(options(&args)?, expected);
    assert!(options(&["--service".to_owned()]).is_err());
    assert!(options(&["--bogus".to_owned()]).is_err());
    Ok(())
}

#[test]
fn passed_tests_are_summed_from_libtest_output() {
    let stdout = concat!(
        "running 1 test\ntest result: ok. 1 passed; 0 failed\n\n",
        "test result: ok. 2 passed; 0 failed\ntest result: ok. 0 passed; 0 failed\n"
    );
    assert_eq!(passed(stdout), 3);
    assert_eq!(passed("nothing"), 0);
}
