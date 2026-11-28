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

/// Over the fake models, a paginated operation whose paginator reads
/// member names, or its token with an expression the native paginator
/// evaluates (`Jobs[-1].JobId`), is admitted with no arguments, and one
/// whose `more_results` compares (``Meta.Count > `0` ``) is refused as a
/// paginator.
#[test]
fn a_paginator_s_standing_follows_what_the_native_one_runs() -> Result<()> {
    let table = &customizations::TABLE[..];
    let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
    let standings = crate::with_source(empty_home()?, Some(models.into_os_string()), |source| {
        [
            ("zjson", "list-things"),
            ("zquery", "list-jobs"),
            ("zpages", "list-compared"),
        ]
        .map(|(command, operation)| standing(source, table, command, operation))
    })?;
    assert_eq!(
        standings,
        [
            Standing::NoArguments,
            Standing::NoArguments,
            Standing::Refused(PythonReasonName("paginator")),
        ]
    );
    Ok(())
}
