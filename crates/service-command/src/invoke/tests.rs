use super::*;

/// The payload's bundle is trusted only for the embedded setting, and a
/// configured file always wins.
#[test]
fn trust_follows_the_setting_and_the_payload() {
    let mut settings = ClientSettings::new("zquery.get-thing");
    settings.trust.value = Trust::Embedded;
    assert_eq!(
        trust(&settings, Some(b"pem")),
        client::transport::crt::Trust::Pem(b"pem".to_vec())
    );
    assert_eq!(
        trust(&settings, None),
        client::transport::crt::Trust::Platform
    );
    settings.trust.value = Trust::Platform;
    assert_eq!(
        trust(&settings, Some(b"pem")),
        client::transport::crt::Trust::Platform
    );
}

/// A scratch directory holding a `credential_process` that records each
/// run in `runs` and fails, and the shared files that configure it.
#[cfg(unix)]
struct FailingProcess {
    directory: std::path::PathBuf,
    runs: std::path::PathBuf,
    files: awscli_config::ParsedFiles,
}

#[cfg(unix)]
impl FailingProcess {
    fn new(name: &str) -> crate::testing::Result<Self> {
        let directory = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory)?;
        let runs = directory.join("runs");
        let script = directory.join("process.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho run >> {}\nexit 1\n", runs.display()),
        )?;
        let mut permissions = std::fs::metadata(&script)?.permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&script, permissions)?;
        let config = format!("[default]\ncredential_process = {}\n", script.display());
        let files = crate::testing::files(&config, "");
        Ok(Self {
            directory,
            runs,
            files,
        })
    }

    fn runs(&self) -> crate::testing::Result<usize> {
        Ok(if self.runs.exists() {
            std::fs::read_to_string(&self.runs)?.lines().count()
        } else {
            0
        })
    }

    /// `argv`, routed and preflighted in the routed environment.
    fn preflight(&self, argv: &[&str]) -> crate::testing::Result<Box<cli_router::Run>> {
        let verdict = crate::testing::route(argv, &[], &self.files)?;
        let cli_router::Verdict::Run(run) = verdict else {
            return Err(format!("not native: {verdict:?}").into());
        };
        assert_eq!(self.preflight_of(&run), Ok(()));
        Ok(run)
    }

    /// The preflight of a routed `run`, in the routed environment.
    fn preflight_of(&self, run: &cli_router::Run) -> Result<(), PythonReason> {
        let env = crate::testing::environment(&[]);
        let variables = std::collections::HashMap::new();
        let preflight = Preflight {
            env: &env,
            variables: &variables,
            shared_files: &self.files,
            globals: &run.globals,
            directory: &self.directory,
            settings: &run.settings,
            output: &run.output,
            stdout_is_tty: false,
        };
        run.invocation.preflight(&preflight)
    }

    /// The message of the failure a preflighted `run` ends with.
    fn run_failure(&self, run: &cli_router::Run) -> crate::testing::Result<String> {
        let env = crate::testing::environment(&[]);
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let mut cx = Context {
            env: &env,
            shared_files: &self.files,
            globals: &run.globals,
            directory: &self.directory,
            stdout: &mut stdout,
            stderr: &mut stderr,
        };
        let failure = run
            .invocation
            .run(&mut cx)
            .err()
            .ok_or("the run succeeded")?;
        Ok(String::from_utf8_lossy(&failure.message).into_owned())
    }
}

#[cfg(unix)]
impl Drop for FailingProcess {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// The preflight runs no credential provider, so nothing it or the client's
/// `prepare` refuses can have run one before Python runs the chain again:
/// a `credential_process` runs first when the client signs.
#[test]
#[cfg(unix)]
fn the_preflight_runs_no_credential_provider() -> crate::testing::Result<()> {
    let process = FailingProcess::new("preflight")?;
    process.preflight(&["zjson", "ping"])?;
    assert_eq!(process.runs()?, 0, "the preflight ran the process");
    Ok(())
}

/// botocore walks the chain as it creates the client, before it validates
/// the input, so a failing provider is reported over invalid input; the
/// process runs once.
#[test]
#[cfg(unix)]
fn a_failing_credential_chain_is_reported_before_invalid_input() -> crate::testing::Result<()> {
    let process = FailingProcess::new("invalid-input")?;
    let run = process.preflight(&["zjson", "strict", "--name", "", "--size", "0"])?;
    let message = process.run_failure(&run)?;
    assert!(
        message.starts_with("Error when retrieving credentials from custom-process"),
        "{message}"
    );
    assert_eq!(process.runs()?, 1);
    Ok(())
}

/// A starting token only Python reads is refused before any provider
/// runs: `{"NextToken":  [[...]]}`, nested 903 deep, past the native
/// reader's limit (base64 `[[[` is `W1tb`, `]]]` is `XV1d`).
#[test]
#[cfg(unix)]
fn a_starting_token_only_python_reads_runs_no_provider() -> crate::testing::Result<()> {
    let process = FailingProcess::new("python-token")?;
    let token = format!(
        "eyJOZXh0VG9rZW4iOiAg{}{}fQ==",
        "W1tb".repeat(301),
        "XV1d".repeat(301)
    );
    let argv = ["zjson", "list-things", "--starting-token", &token];
    let verdict = crate::testing::route(&argv, &[], &process.files)?;
    let cli_router::Verdict::Run(run) = verdict else {
        return Err(format!("not native: {verdict:?}").into());
    };
    assert_eq!(
        process.preflight_of(&run),
        Err(PythonReason::StartingToken),
        "the preflight admitted it"
    );
    assert_eq!(process.runs()?, 0, "the preflight ran the process");
    Ok(())
}

/// botocore creates the client, walking the chain, before the paginator
/// reads the starting token, so a failing provider is reported over a
/// bad one; the process runs once.
#[test]
#[cfg(unix)]
fn a_failing_chain_is_reported_before_a_bad_starting_token() -> crate::testing::Result<()> {
    let process = FailingProcess::new("bad-token")?;
    let argv = [
        "zjson",
        "list-things",
        "--starting-token",
        "does___not___work",
    ];
    let run = process.preflight(&argv)?;
    let message = process.run_failure(&run)?;
    assert!(
        message.starts_with("Error when retrieving credentials from custom-process"),
        "{message}"
    );
    assert_eq!(process.runs()?, 1);
    Ok(())
}

/// A page option beside `--no-paginate` is what
/// `ensure_paging_params_not_set` raises before botocore creates a client:
/// the run reports it, and no provider runs.
#[test]
#[cfg(unix)]
fn a_raised_paging_handler_is_reported_without_a_client() -> crate::testing::Result<()> {
    let process = FailingProcess::new("raised")?;
    let argv = ["zjson", "list-things", "--no-paginate", "--max-items", "2"];
    let run = process.preflight(&argv)?;
    let message = process.run_failure(&run)?;
    assert_eq!(
        message,
        "Cannot specify --no-paginate along with pagination arguments: --max-items"
    );
    assert_eq!(process.runs()?, 0, "a provider ran");
    Ok(())
}

/// Each client's parse hooks keep the timestamp parser they were built
/// with: a second client with another format does not get the first's,
/// as it did from a process-global parser set once.
#[test]
fn each_client_s_hooks_keep_their_own_timestamp_parser() {
    use format::timestamps::TimestampFormat;
    let wire = Value::Integer(1_591_012_800);
    let iso = cli_parse_hooks(TimestampParser::new(TimestampFormat::Iso8601, Some(0)));
    let raw = cli_parse_hooks(TimestampParser::new(TimestampFormat::Wire, Some(0)));
    let first = (iso.timestamp)(&wire);
    let second = (raw.timestamp)(&wire);
    assert_eq!(second, Ok(wire.clone()));
    assert_ne!(first, second);
    assert_eq!((iso.timestamp)(&wire), first);
}
