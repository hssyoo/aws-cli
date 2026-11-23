use super::*;
use std::ffi::{OsStr, OsString};

fn chain<'a>(aws: Option<&'a str>, profile: Profile<'a>, pager: Option<&'a str>) -> Chain<'a> {
    Chain {
        no_cli_pager: false,
        aws_pager: aws.map(OsStr::new),
        cli_pager: profile,
        pager: pager.map(OsStr::new),
    }
}

/// `_construct_cli_pager_chain` and `get_output_stream`.
#[test]
fn the_pager_is_chosen_as_the_cli_chooses_it() {
    let words = |words: &[&str]| {
        Ok(Destination::Pager(
            words.iter().map(|w| (*w).to_owned()).collect(),
        ))
    };
    let cases = [
        (
            chain(Some("most -s"), Profile::Text("p"), Some("x")),
            true,
            words(&["most", "-s"]),
        ),
        (
            chain(Some(""), Profile::Text("p"), Some("x")),
            true,
            Ok(Destination::Stdout),
        ),
        (
            chain(None, Profile::Text("p 'a b'"), Some("x")),
            true,
            words(&["p", "a b"]),
        ),
        (
            chain(None, Profile::Text(""), Some("x")),
            true,
            Ok(Destination::Stdout),
        ),
        (chain(None, Profile::Absent, Some("x")), true, words(&["x"])),
        (
            chain(None, Profile::Absent, Some("")),
            true,
            Ok(Destination::Stdout),
        ),
        (
            chain(None, Profile::Absent, None),
            true,
            words(&[DEFAULT_PAGER]),
        ),
        (
            chain(Some("most"), Profile::Absent, None),
            false,
            Ok(Destination::Stdout),
        ),
        (
            chain(None, Profile::Block, None),
            true,
            Err(NeedsPython::ProfileBlock),
        ),
        (
            chain(Some("a 'b"), Profile::Absent, None),
            true,
            Err(NeedsPython::Split("No closing quotation")),
        ),
        (
            chain(Some("   "), Profile::Absent, None),
            true,
            Err(NeedsPython::NoWords),
        ),
    ];
    for (chain, tty, expected) in cases {
        assert_eq!(destination(&chain, tty), expected, "{chain:?} tty={tty}");
    }
    let disabled = Chain {
        no_cli_pager: true,
        ..chain(Some("most"), Profile::Absent, None)
    };
    assert_eq!(destination(&disabled, true), Ok(Destination::Stdout));
}

/// The environment `OutputStreamFactory` copies inside
/// `original_ld_library_path`, then `LESS`.
#[test]
fn the_pager_environment_is_the_cli_s() {
    let pairs = |pairs: &[(&str, &str)]| -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    };
    let mut env = environment(pairs(&[("A", "1"), ("LD_LIBRARY_PATH", "/x")]));
    env.sort();
    assert_eq!(
        env,
        pairs(&[
            ("A", "1"),
            ("LESS", "FRX"),
            ("PYINSTALLER_RESET_ENVIRONMENT", "1")
        ])
    );
    let mut env = environment(pairs(&[
        ("LESS", "-S"),
        ("LD_LIBRARY_PATH", "/x"),
        ("LD_LIBRARY_PATH_ORIG", "/o"),
        ("PYINSTALLER_RESET_ENVIRONMENT", "0"),
    ]));
    env.sort();
    let expected = pairs(&[
        ("LD_LIBRARY_PATH", "/o"),
        ("LD_LIBRARY_PATH_ORIG", "/o"),
        ("LESS", "-S"),
        ("PYINSTALLER_RESET_ENVIRONMENT", "0"),
    ]);
    assert_eq!(env, expected);
}

/// A fake pager records its input and `LESS`, and the CLI waits for it.
#[test]
fn a_pager_receives_the_output_and_is_awaited() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("format-pager-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("pager.sh");
    let record = dir.join("record");
    let body = format!(
        concat!(
            "#!/bin/sh\ncat > '{0}.in'\nprintf '%s' \"$LESS\" > '{0}.less'\n",
            "sleep 1\necho done > '{0}.done'\n"
        ),
        record.display()
    );
    std::fs::write(&script, body)?;
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    let argv = vec![script.display().to_string()];
    let env = environment(std::env::vars_os().filter(|(k, _)| k != "LESS"));
    let state = cli_core::interrupt::State::default();
    let outcome = run(&argv, &env, b"page me\n", &state)?;
    assert_eq!(outcome, Outcome::Exited);
    let read = |suffix: &str| std::fs::read_to_string(format!("{}.{suffix}", record.display()));
    assert_eq!(read("in")?, "page me\n");
    assert_eq!(read("less")?, "FRX");
    assert_eq!(read("done")?, "done\n");
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// `LazyPager._do_popen`'s `FileNotFoundError` becomes the CLI's
/// `PagerInitializationException`; another failure is a plain `OSError`.
#[test]
fn a_missing_pager_fails_with_python_s_text() {
    let env = environment(std::env::vars_os());
    let state = cli_core::interrupt::State::default();
    let argv = vec!["no-such-pager-for-format-tests".to_owned()];
    let error = run(&argv, &env, b"x", &state).err();
    let expected = "[Errno 2] No such file or directory: 'no-such-pager-for-format-tests'";
    assert_eq!(error.as_ref().map(|e| e.text.as_str()), Some(expected));
    assert_eq!(error.map(|e| e.not_found), Some(true));
}

/// `get_pager_stream` swallows every `OSError` but the missing program's:
/// a pager that cannot be executed leaves the command's output unwritten
/// and its status 0 (`except OSError: pass`; `LazyPager.communicate`
/// starts nothing once the start failed).
#[test]
fn a_pager_that_cannot_run_is_no_failure() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("format-pager-noexec-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("not-executable");
    std::fs::write(&script, "#!/bin/sh\ncat\n")?;
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
    let env = environment(std::env::vars_os());
    let state = cli_core::interrupt::State::default();
    let outcome = run(&[script.display().to_string()], &env, b"x", &state);
    assert_eq!(outcome, Ok(Outcome::Exited));
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// The wait is active work: a Ctrl-C the signal handler records while the
/// pager runs cancels the wait (`Signal::Canceled`) instead of terminating
/// the process, and the run reports it.
#[test]
fn ctrl_c_while_the_pager_runs_cancels_the_wait() -> Result<(), Box<dyn std::error::Error>> {
    let env = environment(std::env::vars_os());
    let state = std::sync::Arc::new(cli_core::interrupt::State::default());
    let signaller = {
        let state = state.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            while state.active_count() == 0 && started.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let signal = state.record_interrupt();
            state.request();
            signal
        })
    };
    let argv = vec!["sleep".to_owned(), "5".to_owned()];
    let outcome = run(&argv, &env, b"", &state);
    let signal = signaller.join().map_err(|_| "signaller panicked")?;
    assert_eq!(signal, cli_core::interrupt::Signal::Canceled);
    assert_eq!(outcome, Ok(Outcome::Interrupted));
    Ok(())
}
