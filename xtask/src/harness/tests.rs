use super::*;
use std::time::{Duration, SystemTime};

fn default(target: fn() -> Result<String, TaskError>) -> Default {
    Default {
        target,
        build: "cargo xtask build",
    }
}

/// Only the default artifact names `deps::TARGET`, so only it takes the
/// static host list; an explicit binary runs on any host.
#[test]
fn only_the_default_artifact_takes_the_static_host_check() -> Result<(), TaskError> {
    fn unsupported() -> Result<String, TaskError> {
        Err(deps::error("unsupported native host"))
    }
    let explicit = std::env::current_exe()?;
    let path = explicit
        .to_str()
        .ok_or_else(|| deps::error("test executable path is not UTF-8"))?;
    let args = ["--aws-binary".to_owned(), path.to_owned()];
    let (root, target) = (Path::new("/workspace"), Path::new("/workspace/target"));
    assert_eq!(
        select_binary(root, target, &args, &default(unsupported))?,
        explicit
    );
    assert_eq!(
        select_binary(root, target, &[], &default(unsupported)).map_err(|e| e.to_string()),
        Err("unsupported native host".to_owned())
    );
    Ok(())
}

/// The default artifact lies under the target directory given, which
/// `binary` takes from `product::target_dir`, and under the triple its
/// target check names, as the build writes it.
#[test]
fn the_default_artifact_lies_under_the_target_directory() {
    fn system() -> Result<String, TaskError> {
        Ok("x86_64-unknown-linux-gnu".to_owned())
    }
    // Joined component by component, as the host spells a path.
    let expected = [
        "/elsewhere",
        "x86_64-unknown-linux-gnu",
        "release-safe",
        "aws",
    ]
    .iter()
    .collect::<PathBuf>()
    .with_extension(std::env::consts::EXE_EXTENSION);
    let selected = select_binary(
        Path::new("/workspace"),
        Path::new("/elsewhere"),
        &[],
        &default(system),
    );
    assert_eq!(
        selected.map_err(|error| error.to_string()),
        Err(format!(
            "Rust CLI is absent: {}; run `cargo xtask build` or supply --aws-binary",
            expected.display()
        ))
    );
}

/// A scratch directory holding a binary built at `built` from the sources
/// `sources` names, each written at its time, and `aws.d` recording them.
struct Built {
    directory: PathBuf,
    binary: PathBuf,
}

impl Built {
    fn new(name: &str, built: SystemTime, sources: &[(&str, SystemTime)]) -> io::Result<Self> {
        let directory =
            std::env::temp_dir().join(format!("xtask-harness-{name}-{}", std::process::id()));
        fs::create_dir_all(&directory)?;
        let binary = directory.join("aws");
        write_at(&binary, built)?;
        let mut record = format!("{}:", binary.display());
        for (source, time) in sources {
            let path = directory.join(source);
            write_at(&path, *time)?;
            record.push(' ');
            record.push_str(&path.display().to_string().replace(' ', "\\ "));
        }
        record.push('\n');
        fs::write(directory.join("aws.d"), record)?;
        Ok(Self { directory, binary })
    }

    fn check(&self) -> Result<(), String> {
        require_current(Path::new("/workspace"), &self.binary, "cargo xtask build")
            .map_err(|error| error.to_string())
    }
}

impl Drop for Built {
    fn drop(&mut self) {
        // A scratch directory left behind is only clutter in the temp dir.
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn write_at(path: &Path, time: SystemTime) -> io::Result<()> {
    fs::write(path, b"")?;
    fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(time)
}

#[test]
fn a_binary_newer_than_its_recorded_sources_is_current() -> Result<(), TaskError> {
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let built = Built::new(
        "current",
        then,
        &[
            ("lib.rs", then),
            ("with space.rs", then - Duration::from_secs(1)),
        ],
    )?;
    assert_eq!(built.check(), Ok(()));
    Ok(())
}

/// A source newer than the binary, a recorded source that is gone, and a
/// binary without its dep-info each fail before any test runs, naming the
/// file and the command that rebuilds it.
#[test]
fn a_stale_or_unrecorded_binary_fails_naming_why_and_the_build() -> Result<(), TaskError> {
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let later = then + Duration::from_secs(60);
    let stale = Built::new("stale", then, &[("lib.rs", then), ("edited rs", later)])?;
    assert_eq!(
        stale.check(),
        Err(format!(
            "Rust CLI is stale: {} was built from {}, which is newer than it; \
             run `cargo xtask build`",
            stale.binary.display(),
            stale.directory.join("edited rs").display()
        ))
    );
    let gone = Built::new("gone", then, &[("removed.rs", then)])?;
    fs::remove_file(gone.directory.join("removed.rs"))?;
    assert_eq!(
        gone.check(),
        Err(format!(
            "Rust CLI is stale: {} was built from {}, which is gone; run `cargo xtask build`",
            gone.binary.display(),
            gone.directory.join("removed.rs").display()
        ))
    );
    let unrecorded = Built::new("unrecorded", then, &[])?;
    fs::remove_file(unrecorded.directory.join("aws.d"))?;
    assert_eq!(
        unrecorded.check(),
        Err(format!(
            "Rust CLI {} has no Cargo dep-info {}, so whether it is current is unknown; \
             run `cargo xtask build`",
            unrecorded.binary.display(),
            unrecorded.directory.join("aws.d").display()
        ))
    );
    Ok(())
}

#[test]
fn dep_info_sources_unescape_spaces_and_read_only_the_first_rule() {
    let text = "/t/aws: /a/b.rs /a/with\\ space.rs  /c.rs\n\n/a/b.rs:\n";
    assert_eq!(
        dep_info_sources(text),
        [
            PathBuf::from("/a/b.rs"),
            PathBuf::from("/a/with space.rs"),
            PathBuf::from("/c.rs")
        ]
    );
    assert!(dep_info_sources("no rule here").is_empty());
}

#[test]
fn selection_and_early_stop_arguments_are_rejected() {
    for args in [
        ["-k", "test_ls"],
        ["--deselect", "test"],
        ["-x", "--run-all"],
    ] {
        assert!(binary(Path::new("/workspace"), &args.map(str::to_owned)).is_err());
    }
}

#[test]
fn junit_distinguishes_expected_failures_from_unexecuted_skips() {
    let xml = concat!(
        "<testsuites><testsuite tests=\"3\" failures=\"0\" errors=\"0\" skipped=\"2\">",
        "<testcase/><testcase><skipped type=\"pytest.xfail\" message=\"known\"/></testcase>",
        "<testcase><skipped type=\"pytest.skip\" message=\"platform\"/></testcase>",
        "</testsuite></testsuites>"
    );
    assert_eq!(
        counts(xml).ok(),
        Some(Counts {
            tests: 3,
            failures: 0,
            errors: 0,
            skipped: 1,
            xfailed: 1,
            native_parity_failures: 0,
        })
    );
}

#[test]
fn junit_reports_native_parity_separately_from_shared_harness_failures() -> Result<(), TaskError> {
    let xml = concat!(
        "<testsuite tests=\"3\" failures=\"0\" errors=\"0\" skipped=\"2\">",
        "<testcase><skipped type=\"pytest.xfail\" message=\"parity-gap: output\"/>",
        "</testcase><testcase><skipped type=\"pytest.xfail\" ",
        "message=\"shared-harness-failure: local mock\"/></testcase><testcase/>",
        "</testsuite>"
    );
    let actual = counts(xml)?;
    assert_eq!(actual.native_parity_failures, 1);
    assert_eq!(actual.xfailed, 2);
    Ok(())
}

#[test]
fn malformed_junit_does_not_silently_produce_passing_counts() {
    for xml in [
        "",
        "<testsuite",
        "<testsuite tests=\"bad\">",
        "<testsuite skipped=\"0\">",
    ] {
        assert!(counts(xml).is_err());
    }
}

#[test]
fn self_tests_run_through_their_runner_and_preserve_home() {
    let root = Path::new("/workspace");
    let forwarded = ["-k".to_owned(), "payload".to_owned()];
    let command = self_test_command(root, Some(Path::new("/toolchain/bin/cc")), &forwarded);
    assert_eq!(command.get_program(), "python3");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(
        args,
        [
            "-B".as_ref(),
            root.join("tests/deps/run.py").as_os_str(),
            "--cache-dir".as_ref(),
            root.join("target/harness-test").as_os_str(),
            "-k".as_ref(),
            "payload".as_ref(),
        ]
    );
    assert!(!command.get_envs().any(|(key, _)| key == "HOME"));
    assert!(
        command
            .get_envs()
            .any(|(key, value)| key == "AWS_NATIVE_TEST_CC"
                && value == Some("/toolchain/bin/cc".as_ref()))
    );
    let without = self_test_command(root, None, &[]);
    assert!(
        !without
            .get_envs()
            .any(|(key, _)| key == "AWS_NATIVE_TEST_CC")
    );
}
