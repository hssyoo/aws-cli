//! Starting a program as CPython's `subprocess` starts one on POSIX.
//!
//! `std::process::Command` starts its program with `posix_spawnp`, or with
//! `execvp` after a fork. On macOS both run a file the kernel refuses as
//! not executable code (`ENOEXEC`) through `/bin/sh`, and glibc's `execvp`
//! does too. CPython's child calls `execv` on each candidate in turn and
//! moves past one that fails (`_posixsubprocess`'s `child_exec`), so it
//! never runs such a file.
use std::ffi::CString;
use std::io;
use std::iter;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Runs the first of `candidates` the kernel starts, as `argv0` with
/// `arguments` and this process's standard input, and waits for it to
/// exit with its output captured.
///
/// Each candidate is passed to `execv` in turn in the child, as CPython's
/// `_posixsubprocess` passes the executable list `subprocess` builds, with
/// this process's environment.
///
/// # Errors
/// `InvalidInput` when a candidate or a word of the command holds a NUL
/// byte, before anything starts. When no candidate starts, the first
/// failure other than `ENOENT` or `ENOTDIR`, or else the last candidate's,
/// which is the error `subprocess` raises. A failure to create the pipes
/// or the child.
pub fn output<P: AsRef<Path>>(
    candidates: &[P],
    argv0: &str,
    arguments: &[String],
) -> io::Result<Output> {
    let exec = Exec::new(candidates, argv0, arguments)?;
    let Some(first) = candidates.first() else {
        return Err(io::ErrorKind::NotFound.into());
    };
    // `Command` never execs its own program: the closure either replaces
    // the child or returns the failure, which `Command` reports.
    let mut command = Command::new(first.as_ref());
    command.stdin(Stdio::inherit());
    // SAFETY: the closure runs in the forked child, where only
    // async-signal-safe functions may be called. It calls only `execv` and
    // reads `errno`, and it allocates nothing: `Exec::new` built every
    // string and the `argv` array before the fork.
    unsafe { command.pre_exec(move || Err(exec.run())) };
    command.output()
}

/// CPython's default search path, `os.defpath`.
const DEFAULT_PATH: &str = "/bin:/usr/bin";

/// The files CPython's `subprocess` tries for `program`
/// (`Popen._execute_child`): a name with a directory as given, any other
/// joined to each directory of `path` (`os.get_exec_path`, `os.defpath`
/// when unset), an empty directory leaving the bare name, which `execv`
/// finds in the working directory.
pub fn candidates(program: &str, path: Option<&std::ffi::OsStr>) -> Vec<std::path::PathBuf> {
    if program.contains('/') {
        return vec![program.into()];
    }
    let path = path.unwrap_or(std::ffi::OsStr::new(DEFAULT_PATH));
    std::env::split_paths(path)
        .map(|directory| directory.join(program))
        .collect()
}

/// Python's `shlex.split(command)`: POSIX quoting, whitespace between
/// words, no comments. botocore's `compat_shell_split` splits a
/// `credential_process` with it off Windows, and awscli's
/// `get_popen_kwargs_for_pager_cmd` splits the pager command with it.
///
/// # Errors
/// The text `shlex` raises `ValueError` with: a quotation that is never
/// closed, or a backslash that ends the command.
pub fn shell_split(command: &str) -> Result<Vec<String>, &'static str> {
    const NO_CLOSING_QUOTATION: &str = "No closing quotation";
    const NO_ESCAPED_CHARACTER: &str = "No escaped character";
    let mut words = Vec::new();
    // `Some` from a word's first character or quote, so `''` is a word.
    let mut word: Option<String> = None;
    let mut characters = command.chars();
    while let Some(character) = characters.next() {
        match character {
            ' ' | '\t' | '\r' | '\n' => words.extend(word.take()),
            '\\' => word
                .get_or_insert_default()
                .push(characters.next().ok_or(NO_ESCAPED_CHARACTER)?),
            '\'' => {
                let word = word.get_or_insert_default();
                loop {
                    match characters.next().ok_or(NO_CLOSING_QUOTATION)? {
                        '\'' => break,
                        quoted => word.push(quoted),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_default();
                loop {
                    match characters.next().ok_or(NO_CLOSING_QUOTATION)? {
                        '"' => break,
                        '\\' => {
                            // Inside double quotes a backslash escapes only
                            // itself and the quote; before anything else it
                            // stays.
                            let escaped = characters.next().ok_or(NO_ESCAPED_CHARACTER)?;
                            if !matches!(escaped, '\\' | '"') {
                                word.push('\\');
                            }
                            word.push(escaped);
                        }
                        quoted => word.push(quoted),
                    }
                }
            }
            plain => word.get_or_insert_default().push(plain),
        }
    }
    words.extend(word);
    Ok(words)
}

/// `execv`'s arguments, built before the fork.
struct Exec {
    candidates: Vec<CString>,
    /// The NULL-terminated `argv` array, which points into `_words`.
    argv: Vec<*const libc::c_char>,
    /// Owns the strings `argv` points at.
    _words: Vec<CString>,
}

// SAFETY: `argv` points only into the heap buffers of `_words`, which the
// `Exec` owns and never changes, so moving it to another thread hands over
// nothing it does not own.
unsafe impl Send for Exec {}
// SAFETY: an `Exec` is never changed after it is built, and the pointers in
// `argv` are only read.
unsafe impl Sync for Exec {}

impl Exec {
    fn new<P: AsRef<Path>>(
        candidates: &[P],
        argv0: &str,
        arguments: &[String],
    ) -> io::Result<Self> {
        let candidates = candidates
            .iter()
            .map(|candidate| c_string(candidate.as_ref().as_os_str().as_bytes()))
            .collect::<io::Result<Vec<_>>>()?;
        let words = iter::once(argv0)
            .chain(arguments.iter().map(String::as_str))
            .map(|word| c_string(word.as_bytes()))
            .collect::<io::Result<Vec<_>>>()?;
        let argv = words
            .iter()
            .map(|word| word.as_ptr())
            .chain(iter::once(std::ptr::null()))
            .collect();
        Ok(Self {
            candidates,
            argv,
            _words: words,
        })
    }

    /// Calls `execv` on each candidate, which returns only when it fails,
    /// and returns the failure `subprocess` reports.
    fn run(&self) -> io::Error {
        let mut first = None;
        let mut last = io::Error::from_raw_os_error(libc::ENOENT);
        for candidate in &self.candidates {
            // SAFETY: `candidate` is NUL-terminated, and `argv` is a
            // NULL-terminated array of NUL-terminated strings, all of which
            // `self` keeps alive through the call.
            unsafe { libc::execv(candidate.as_ptr(), self.argv.as_ptr()) };
            let failure = io::Error::last_os_error();
            let missing = matches!(failure.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR));
            if first.is_none() && !missing {
                first = Some(failure);
            } else {
                last = failure;
            }
        }
        first.unwrap_or(last)
    }
}

/// `bytes` as a C string, refused as `Command` refuses a NUL byte.
fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "nul byte found in provided data",
        )
    })
}

#[cfg(test)]
mod tests;
