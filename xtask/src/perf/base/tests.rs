use super::*;

#[test]
fn a_built_base_is_kept_under_target_by_commit() {
    let root = Path::new("/work/aws-cli");
    let binary = cached(root, "abc123");
    let expected = Path::new("/work/aws-cli/target/perf/base/abc123/aws")
        .with_extension(std::env::consts::EXE_EXTENSION);
    assert_eq!(binary, expected);
}

#[test]
fn an_absent_base_binary_is_refused() {
    let options = Options {
        base_binary: Some("absent/aws".into()),
        ..Options::default()
    };
    let error = resolve(Path::new("/nowhere"), &options).err();
    assert_eq!(
        error.map(|error| error.to_string()),
        Some("base binary is absent: /nowhere/absent/aws".to_owned())
    );
}

#[test]
fn a_main_line_named_by_option_is_used_without_asking_git() -> Result<(), TaskError> {
    let options = Options {
        mainline: Some("main".into()),
        ..Options::default()
    };
    assert_eq!(
        mainline(Path::new("/nowhere"), &options)?,
        ("main".to_owned(), "--mainline")
    );
    Ok(())
}
