use super::*;

/// One Cargo artifact message for `executable`, a test when `test`, built
/// from the package manifest `manifest`, if any.
fn artifact(executable: &str, test: bool, manifest: Option<serde_json::Value>) -> String {
    let mut message = serde_json::json!({
        "reason": "compiler-artifact",
        "profile": {"test": test},
        "executable": executable,
    });
    if let Some(manifest) = manifest {
        message["manifest_path"] = manifest;
    }
    message.to_string()
}

#[test]
fn artifact_selection_keeps_both_interpreter_harnesses_and_unit_tests() -> Result<(), TaskError> {
    let fallback = || Some("/w/python-fallback/Cargo.toml".into());
    let unit = || Some("/w/crates/unit/Cargo.toml".into());
    let bytes = [
        artifact("/finder", true, fallback()),
        artifact("/exit-status", true, fallback()),
        artifact("/unit", true, unit()),
        artifact("/build-script", false, unit()),
        r#"{"reason":"build-finished","success":true}"#.to_owned(),
    ]
    .join("\n");
    assert_eq!(
        test_artifacts(bytes.as_bytes())?,
        [
            ("/exit-status", "/w/python-fallback"),
            ("/finder", "/w/python-fallback"),
            ("/unit", "/w/crates/unit"),
        ]
        .map(|(path, package)| TestArtifact {
            path: PathBuf::from(path),
            package: PathBuf::from(package),
        })
    );
    assert!(test_artifacts(b"{}").is_err());
    Ok(())
}

/// A test executable runs in its package directory, as under `cargo test`,
/// so a test without a package fails the run rather than resolving its
/// fixtures elsewhere.
#[test]
fn artifact_selection_requires_each_test_executables_package() {
    for manifest in [None, Some(7.into()), Some("".into()), Some("/".into())] {
        let line = artifact("/unit", true, manifest);
        let message = test_artifacts(line.as_bytes())
            .err()
            .map(|failure| failure.to_string())
            .unwrap_or_default();
        assert_eq!(message, "test executable /unit has no package", "{line}");
    }
}

fn unit_artifact() -> TestArtifact {
    TestArtifact {
        path: PathBuf::from("/unit"),
        package: PathBuf::from("/workspace/crates/unit"),
    }
}

#[test]
fn test_runs_pass_the_filter_first_under_the_build_environment() {
    let mut build = Command::new("cargo");
    build.env("AWS_NATIVE_CRT_PREFIX", "/crt");
    let profile = Path::new("/coverage/profiles/%p-%m.profraw");
    let filtered = test_command(&unit_artifact(), &build, profile, Some("scheduler"));
    assert_eq!(filtered.get_program(), "/unit");
    assert_eq!(filtered.get_args().collect::<Vec<_>>(), ["scheduler"]);
    assert_eq!(
        filtered.get_current_dir(),
        Some(Path::new("/workspace/crates/unit"))
    );
    let environment = filtered.get_envs().collect::<Vec<_>>();
    assert!(environment.contains(&("AWS_NATIVE_CRT_PREFIX".as_ref(), Some("/crt".as_ref()))));
    assert!(environment.contains(&("LLVM_PROFILE_FILE".as_ref(), Some(profile.as_os_str()))));
    let unfiltered = test_command(&unit_artifact(), &build, profile, None);
    assert_eq!(unfiltered.get_args().count(), 0);
}

/// Coverage runs the native suite, whose compiler probes fail rather than
/// skip without the managed compilers, so every test run names them.
#[test]
fn test_runs_name_the_managed_compilers() {
    let mut tools = crate::deps::keying_toolchain();
    tools.cc = PathBuf::from("/managed/bin/cc");
    tools.clang = PathBuf::from("/managed/bin/clang");
    let mut build = Command::new("cargo");
    apply_test_compilers(&tools, &mut build);
    let run = test_command(
        &unit_artifact(),
        &build,
        Path::new("/coverage/profiles/%p-%m.profraw"),
        None,
    );
    let environment = run.get_envs().collect::<Vec<_>>();
    for (name, value) in [
        ("AWS_NATIVE_TEST_CC", "/managed/bin/cc"),
        ("AWS_NATIVE_TEST_CLANG", "/managed/bin/clang"),
    ] {
        assert!(
            environment.contains(&(name.as_ref(), Some(value.as_ref()))),
            "{name}: {environment:?}"
        );
    }
}

/// A failing test executable fails the run, and every failure is named.
#[test]
fn failed_runs_name_every_failing_test_executable() {
    assert_eq!(
        failed_runs(&[
            "command \"/a\" failed".to_owned(),
            "command \"/b\" failed".to_owned()
        ]),
        "coverage: 2 test executables failed:\ncommand \"/a\" failed\ncommand \"/b\" failed"
    );
}
