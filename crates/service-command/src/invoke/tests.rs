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

/// A scratch directory, removed when dropped.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> crate::testing::Result<Self> {
        let directory = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory)?;
        Ok(Self(directory))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A streaming blob member's path is expanded as `unpack_scalar_cli_arg`
/// expands it (variables, then `~`) and read relative to the working
/// directory; anything but a regular file native reads whole is Python's:
/// a missing path, a directory, a file beyond the cap, a `~user` path.
#[test]
#[cfg(unix)]
fn an_input_file_is_read_as_unpack_scalar_cli_arg_opens_it() -> crate::testing::Result<()> {
    let scratch = Scratch::new("input-file")?;
    let directory = &scratch.0;
    std::fs::write(directory.join("body.bin"), b"\x00body\xff")?;
    std::fs::write(directory.join("empty"), b"")?;
    std::fs::create_dir(directory.join("sub"))?;
    let large = std::fs::File::create(directory.join("large"))?;
    large.set_len(INPUT_FILE_BYTES_MAX + 1)?;
    let variables = std::collections::HashMap::from([
        ("NAME".to_owned(), OsString::from("body.bin")),
        ("HOME".to_owned(), directory.clone().into_os_string()),
        // expandvars runs first, so a `~` it yields is the home's.
        ("TILDE".to_owned(), OsString::from("~/body.bin")),
    ]);
    let read = |path: &str| read_input_file(path, &variables, directory);
    assert_eq!(read("body.bin").as_deref(), Some(&b"\x00body\xff"[..]));
    assert_eq!(read("$NAME").as_deref(), Some(&b"\x00body\xff"[..]));
    assert_eq!(read("${NAME}").as_deref(), Some(&b"\x00body\xff"[..]));
    assert_eq!(read("~/body.bin").as_deref(), Some(&b"\x00body\xff"[..]));
    assert_eq!(read("$TILDE").as_deref(), Some(&b"\x00body\xff"[..]));
    let absolute = directory.join("body.bin");
    let absolute = absolute.to_str().ok_or("path")?;
    assert_eq!(read(absolute).as_deref(), Some(&b"\x00body\xff"[..]));
    assert_eq!(read("empty").as_deref(), Some(&b""[..]));
    for refused in [
        "missing",
        "sub",
        "large",
        "$UNSET/body.bin",
        "~nobody/body.bin",
        "",
    ] {
        assert_eq!(read(refused), None, "{refused}");
    }
    Ok(())
}

/// `save_file`'s `open(path, 'wb')`: the bytes replace whatever the file
/// held; an open that fails is `str()` of its `OSError`, the path as given.
#[test]
#[cfg(unix)]
fn the_outfile_is_written_or_fails_as_open_raises() -> crate::testing::Result<()> {
    let scratch = Scratch::new("outfile")?;
    let directory = &scratch.0;
    std::fs::write(directory.join("out.bin"), b"a longer old body")?;
    write_outfile("out.bin", directory, b"\x00new").map_err(|failure| format!("{failure:?}"))?;
    assert_eq!(std::fs::read(directory.join("out.bin"))?, b"\x00new");
    let failed = |path: &str| {
        let failure = write_outfile(path, directory, b"x").err();
        failure.map(|failure| {
            assert_eq!(failure.category, cli_core::cli_error::Category::General);
            String::from_utf8_lossy(&failure.message).into_owned()
        })
    };
    assert_eq!(
        failed("no-dir/out.bin").as_deref(),
        Some("[Errno 2] No such file or directory: 'no-dir/out.bin'")
    );
    assert_eq!(
        failed(".").as_deref(),
        Some("[Errno 21] Is a directory: '.'")
    );
    assert_eq!(
        failed("").as_deref(),
        Some("[Errno 2] No such file or directory: ''")
    );
    assert_eq!(
        failed("it's/x").as_deref(),
        Some("[Errno 2] No such file or directory: \"it's/x\"")
    );
    Ok(())
}

/// `str()` of an `OSError` without a file name: the number and the C
/// library's text, without Rust's ` (os error N)`.
#[test]
fn an_os_error_reads_as_python_s() {
    assert_eq!(
        os_error(&std::io::Error::from_raw_os_error(28)),
        "[Errno 28] No space left on device"
    );
    let other = std::io::Error::other("custom");
    assert_eq!(os_error(&other), "custom");
}
