use super::*;

type Result<T> = std::result::Result<T, crate::Error>;

/// An empty home directory, so no `~/.aws/models` joins the shipped ones.
fn empty_home() -> Result<std::ffi::OsString> {
    let home = std::env::temp_dir().join(format!("model-tools-home-{}", std::process::id()));
    std::fs::create_dir_all(&home)?;
    Ok(home.into_os_string())
}

/// Over the shipped models alone (no `AWS_DATA_PATH`), sts's caller
/// identity is admitted with no arguments, and the census report states
/// its total and one line per standing.
#[test]
fn a_shipped_operation_has_its_standing_and_the_report_its_rows() -> Result<()> {
    let table = &customizations::TABLE[..];
    let standing = crate::with_source(empty_home()?, None, |source| {
        standing(source, table, "sts", "get-caller-identity")
    })?;
    assert_eq!(standing, Standing::NoArguments);
    let counts = BTreeMap::from([
        (Standing::NoArguments, 2),
        (
            Standing::Refused(PythonReasonName("unsupported_protocol")),
            3,
        ),
    ]);
    assert_eq!(
        report(&counts),
        concat!(
            "5 operations\n     2 NoArguments\n",
            "     3 Refused(PythonReasonName(\"unsupported_protocol\"))\n"
        )
    );
    Ok(())
}
