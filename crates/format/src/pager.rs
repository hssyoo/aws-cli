//! The output pager: `OutputStreamFactory.get_output_stream` and
//! `get_pager_stream`, the `cli_pager` chain
//! (`CLIDriver._construct_cli_pager_chain`) and `LazyPager`.
//!
//! The command is decided before any request: [`destination`] gives the
//! pager's words, stdout, or a [`NeedsPython`] reason for the cases the
//! Python route fails in ways this module does not reproduce. The pager
//! starts only when output is written, as `LazyPager` starts it, and the
//! CLI waits for it to exit (`process.communicate()`); its status is ignored.
//! A paginated response's text reaches it page by page ([`Stream`]).
use cli_core::interrupt::State;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::time::{Duration, Instant};

/// `compat.default_pager` off Windows.
pub const DEFAULT_PAGER: &str = "less";
/// `OutputStreamFactory`'s `default_less_flags`.
const LESS_DEFAULT: &str = "FRX";
/// How long `Popen.communicate` waits for the child after a Ctrl-C
/// (`Popen._sigint_wait_secs`) before raising `KeyboardInterrupt`.
const SIGINT_WAIT: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(10);

/// The selected profile's `cli_pager`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile<'a> {
    Absent,
    Text(&'a str),
    /// A nested block, which Python reads as a dict.
    Block,
}

/// The sources of the `pager` setting, in the chain's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chain<'a> {
    /// `--no-cli-pager` (`no_pager_handler`), which clears the setting.
    pub no_cli_pager: bool,
    pub aws_pager: Option<&'a OsStr>,
    pub cli_pager: Profile<'a>,
    pub pager: Option<&'a OsStr>,
}

/// Where a command's output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Stdout,
    /// The pager's words, `shlex.split` of the command.
    Pager(Vec<String>),
}

/// A pager setting only the Python route handles, known before any request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedsPython {
    /// A `cli_pager` block: `shlex.split` of a dict raises.
    ProfileBlock,
    /// A value that is not Unicode, which Python carries with surrogates.
    NotUnicode,
    /// `shlex.split` raises `ValueError` with this text.
    Split(&'static str),
    /// A command of only whitespace: `Popen([])` raises `IndexError`.
    NoWords,
    /// On Windows the pager runs through the shell (`shell=True`).
    Windows,
}

/// `get_output_stream`: the pager when stdout is a terminal and the
/// setting is a non-empty command.
pub fn destination(chain: &Chain<'_>, stdout_is_tty: bool) -> Result<Destination, NeedsPython> {
    if !stdout_is_tty {
        return Ok(Destination::Stdout);
    }
    let Some(command) = configured(chain)? else {
        return Ok(Destination::Stdout);
    };
    if command.is_empty() {
        return Ok(Destination::Stdout);
    }
    if cfg!(windows) {
        return Err(NeedsPython::Windows);
    }
    let words = cli_core::subprocess::shell_split(command).map_err(NeedsPython::Split)?;
    if words.is_empty() {
        return Err(NeedsPython::NoWords);
    }
    Ok(Destination::Pager(words))
}

/// The chain's value: `AWS_PAGER` when set, even empty, then the profile's
/// `cli_pager`, then `PAGER`, then the default; `None` with `--no-cli-pager`.
fn configured<'a>(chain: &Chain<'a>) -> Result<Option<&'a str>, NeedsPython> {
    if chain.no_cli_pager {
        return Ok(None);
    }
    let text = |value: &'a OsStr| value.to_str().ok_or(NeedsPython::NotUnicode);
    if let Some(value) = chain.aws_pager {
        return text(value).map(Some);
    }
    match chain.cli_pager {
        Profile::Text(value) => return Ok(Some(value)),
        Profile::Block => return Err(NeedsPython::ProfileBlock),
        Profile::Absent => {}
    }
    chain
        .pager
        .map_or(Ok(Some(DEFAULT_PAGER)), |value| text(value).map(Some))
}

/// The pager's environment: this process's, as `original_ld_library_path`
/// leaves it while `OutputStreamFactory` copies it, with `LESS` defaulted.
pub fn environment(
    env: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = env.into_iter().collect();
    let position =
        |env: &[(OsString, OsString)], name: &str| env.iter().position(|(k, _)| k == name);
    if position(&env, "PYINSTALLER_RESET_ENVIRONMENT").is_none() {
        env.push(("PYINSTALLER_RESET_ENVIRONMENT".into(), "1".into()));
    }
    let original = position(&env, "LD_LIBRARY_PATH_ORIG").map(|index| env[index].1.clone());
    env.retain(|(name, _)| name != "LD_LIBRARY_PATH");
    if let Some(original) = original {
        env.push(("LD_LIBRARY_PATH".into(), original));
    }
    if position(&env, "LESS").is_none() {
        env.push(("LESS".into(), LESS_DEFAULT.into()));
    }
    env
}

/// How waiting for the pager ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The pager exited; its status is ignored.
    Exited,
    /// A Ctrl-C arrived while waiting: Python waits up to 0.25 s more, then
    /// its `KeyboardInterrupt` handler writes `\n` to stdout and exits 130,
    /// leaving the pager running.
    Interrupted,
}

/// The pager could not start because its program is missing
/// (`FileNotFoundError`), or its command is empty (`IndexError`); any other
/// `OSError` is no failure (see [`run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnError {
    /// `str()` of the exception, `[Errno N] text: 'name'`.
    pub text: String,
    /// A `FileNotFoundError`, which `LazyPager` turns into
    /// `PagerInitializationException` (`PagerErrorHandler`, exit 253, code
    /// `Pager`); an empty command's `IndexError` is the general handler's
    /// (exit 255).
    pub not_found: bool,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for SpawnError {}

/// Starts the pager with `argv` and `env`, writes `output` to it, closes
/// its input and waits for it. A failed write, such as a pager that quit
/// early, is ignored, as the CLI ignores the `OSError`. Call only with
/// output to write: an unwritten `LazyPager` never starts.
///
/// # Errors
/// See [`SpawnError`].
pub fn run(
    argv: &[String],
    env: &[(OsString, OsString)],
    output: &[u8],
    interrupt: &State,
) -> Result<Outcome, SpawnError> {
    let mut stream = Stream::new(argv, env);
    stream.write(output)?;
    Ok(stream.finish(interrupt))
}

/// `io.DEFAULT_BUFFER_SIZE`, the buffer of the pager's input pipe
/// (`Popen(stdin=PIPE)` with the default `bufsize`).
const PIPE_BUFFER: usize = 128 * 1024;

/// The pager as `get_pager_stream` writes to it, one piece of output at a
/// time: `LazyPager` starts it at the first write, its input is buffered as
/// Python's pipe is, and the first `OSError`, from starting it or writing
/// to it, ends the writing, which `get_pager_stream` passes over.
#[derive(Debug)]
pub struct Stream<'a> {
    argv: &'a [String],
    env: &'a [(OsString, OsString)],
    child: Option<std::process::Child>,
    input: Option<std::io::BufWriter<std::process::ChildStdin>>,
    broken: bool,
}

impl<'a> Stream<'a> {
    /// A pager not yet started.
    pub const fn new(argv: &'a [String], env: &'a [(OsString, OsString)]) -> Self {
        Self {
            argv,
            env,
            child: None,
            input: None,
            broken: false,
        }
    }

    /// `LazyStdin.write`: starts the pager the first time, then writes
    /// `output`. False once the pager could not start for an `OSError`
    /// other than a missing program, or its input failed, after which
    /// nothing more is written.
    ///
    /// # Errors
    /// See [`SpawnError`].
    pub fn write(&mut self, output: &[u8]) -> Result<bool, SpawnError> {
        if self.broken {
            return Ok(false);
        }
        if self.child.is_none() {
            let Some(mut child) = self.spawn()? else {
                self.broken = true;
                return Ok(false);
            };
            self.input = child
                .stdin
                .take()
                .map(|stdin| std::io::BufWriter::with_capacity(PIPE_BUFFER, stdin));
            self.child = Some(child);
        }
        let written = self
            .input
            .as_mut()
            .is_some_and(|input| input.write_all(output).is_ok());
        self.broken = !written;
        Ok(written)
    }

    /// `Popen(argv)`; `None` for an `OSError` `get_pager_stream` passes over.
    fn spawn(&self) -> Result<Option<std::process::Child>, SpawnError> {
        let (program, arguments) = self.argv.split_first().ok_or_else(|| SpawnError {
            text: "list index out of range".into(),
            not_found: false,
        })?;
        let path = self
            .env
            .iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value.as_os_str());
        let candidates = cli_core::subprocess::candidates(program, path);
        match cli_core::subprocess::spawn_piped(&candidates, program, arguments, self.env) {
            Ok(child) => Ok(Some(child)),
            // `get_pager_stream` passes over any other `OSError`: nothing
            // is written and the command succeeds.
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(spawn_error(&error, program)),
        }
    }

    /// `process.communicate()`: closes the pager's input and waits for it
    /// to exit; nothing to wait for when it never started.
    pub fn finish(mut self, interrupt: &State) -> Outcome {
        if let Some(mut input) = self.input.take() {
            // communicate() passes over a pipe the pager already closed.
            let _ = input.flush();
        }
        let Some(mut child) = self.child.take() else {
            return Outcome::Exited;
        };
        // Waiting is active work, so the first Ctrl-C cancels it rather than
        // terminating the process (`interrupt::State::record_interrupt`).
        let _scope = cli_core::interrupt::Scope::enter(interrupt);
        loop {
            if !matches!(child.try_wait(), Ok(None)) {
                return Outcome::Exited;
            }
            if interrupt.sleep(POLL).is_err() {
                let deadline = Instant::now() + SIGINT_WAIT;
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                    std::thread::sleep(POLL);
                }
                return Outcome::Interrupted;
            }
        }
    }
}

/// `str(OSError(errno, os.strerror(errno), filename))`.
fn spawn_error(error: &std::io::Error, program: &str) -> SpawnError {
    let errno = error.raw_os_error().unwrap_or(0);
    let description = error.to_string();
    let strerror = description
        .strip_suffix(&format!(" (os error {errno})"))
        .unwrap_or(&description);
    let filename = model::pyrepr::py_repr(&model::value::Value::String(program.into()));
    SpawnError {
        text: format!("[Errno {errno}] {strerror}: {filename}"),
        not_found: error.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests;
