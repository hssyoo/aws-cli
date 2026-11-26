//! `cargo xtask guard --base <rev> [--head <rev>]`: the repository-generic
//! checks a change must pass so that it cannot weaken the oracles it is
//! judged by. Each violation is one line on stdout, with what to do about
//! it, and any violation fails the run. The rules ([`rules`]) are:
//!
//! - `protected`: a change to a path in `xtask/guard/protected.txt`, read
//!   from base, head and the running checkout alike, so a change cannot
//!   drop its own entry and a range from before the list is still checked.
//! - `suppression`: an added attribute that silences a check, wherever it
//!   sits on its line and inside `cfg_attr(..)`: `allow`, `expect`,
//!   `ignore`, `should_panic`, or a `cfg` that is false in a test build
//!   (`cfg(any())`, `cfg(not(test))`), with no justification. A
//!   justification is a `//` comment on the attribute's line or directly
//!   above its stack of attributes, or the attribute's own non-empty
//!   `reason = ".."`, `#[ignore = ".."]` or `expected = ".."`.
//! - `test-count`: fewer test functions in a crate (the deepest directory
//!   holding a `Cargo.toml`) at head than at base, counted in the changed
//!   files, with comments and literals masked out.
//! - `deleted-test`: a test file (`tests.rs`, `testing.rs`, or any file
//!   under a `tests` directory) deleted or renamed to a non-test path,
//!   unless the commit that removes it names its path in its message or in
//!   a `Retires:` trailer.
//! - `test-module`: a removed `mod tests` or `mod testing` declaration,
//!   unless it reappears in the file or a `Retires:` trailer of the commit
//!   names the file or the module's file.
//! - `assert`: an assert statement (an identifier containing `assert` and
//!   its bracketed arguments, however many lines they span) in a test file
//!   that survives at head, with a removed line in it and no identical
//!   statement at head, unless a commit touching the file has a `Retires:`
//!   or `Asserts-changed:` trailer for it.
//!
//! Trailer values are exact paths, separated by commas or spaces, read from
//! the trailer block of a commit that changes the path. The guard counts
//! and compares text: a test emptied of its checks, or one whose checks
//! live in helpers without `assert` in their name, is not caught.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::TaskError;

mod attrs;
mod diff;
#[cfg(test)]
mod evasions;
mod lex;
mod rules;

use diff::{Change, Commit, FileLines};

const PROTECTED_LIST: &str = "xtask/guard/protected.txt";

/// What the rules read, gathered from git once.
#[derive(Debug, Default)]
pub struct Inputs {
    pub protected: Vec<String>,
    pub changes: Vec<Change>,
    pub lines: Vec<FileLines>,
    /// The text at base and at head of each changed `.rs` or test file.
    pub base_sources: BTreeMap<String, String>,
    pub head_sources: BTreeMap<String, String>,
    pub base_crates: Vec<String>,
    pub head_crates: Vec<String>,
    pub commits: Vec<Commit>,
}

pub fn run(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let (base, head) = options(args)?;
    let git = Git { root };
    let base = git.commit(&base)?;
    let head = git.commit(&head)?;
    let inputs = git.inputs(&base, &head)?;
    let violations = rules::check(&inputs);
    let mut stdout = io::stdout().lock();
    for violation in &violations {
        writeln!(stdout, "{violation}")?;
    }
    if violations.is_empty() {
        writeln!(stdout, "guard: {base}..{head}: no violations")?;
        return Ok(());
    }
    Err(TaskError::Message(format!(
        "guard: {} violation(s) in {base}..{head}",
        violations.len()
    )))
}

fn options(args: &[String]) -> Result<(String, String), TaskError> {
    let usage =
        || TaskError::Message("usage: cargo xtask guard --base <rev> [--head <rev>]".into());
    let mut base = None;
    let mut head = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let slot = match arg.as_str() {
            "--base" => &mut base,
            "--head" => &mut head,
            _ => return Err(usage()),
        };
        if slot.is_some() {
            return Err(usage());
        }
        *slot = Some(iter.next().ok_or_else(usage)?.clone());
    }
    Ok((
        base.ok_or_else(usage)?,
        head.unwrap_or_else(|| "HEAD".into()),
    ))
}

/// The settings every git call overrides, so a user's or a repository's
/// configuration cannot change what the guard parses.
const GIT_SETTINGS: [&str; 8] = [
    "core.quotePath=false",
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.relative=false",
    "diff.suppressBlankEmpty=false",
    "log.showSignature=false",
    "log.follow=false",
    "core.attributesFile=/dev/null",
];

/// The flags of every content diff: every file is text (a `-diff` or
/// `binary` attribute cannot hide lines), with no converter or external
/// tool, fixed prefixes and the whole tree.
const DIFF_FLAGS: [&str; 8] = [
    "--text",
    "--no-textconv",
    "--no-ext-diff",
    "--no-color",
    "--no-relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "-M",
];

struct Git<'a> {
    root: &'a Path,
}

impl Git<'_> {
    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(self.root);
        for setting in GIT_SETTINGS {
            command.args(["-c", setting]);
        }
        command
    }

    fn read(&self, args: &[&str]) -> Result<String, TaskError> {
        let output = self.command().args(args).output()?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        Err(TaskError::Message(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    fn commit(&self, rev: &str) -> Result<String, TaskError> {
        let spec = format!("{rev}^{{commit}}");
        Ok(self
            .read(&["rev-parse", "--verify", "--quiet", &spec])?
            .trim()
            .to_owned())
    }

    /// The text of each `rev:path` that exists, by `cat-file --batch`.
    fn blobs(&self, specs: &[String]) -> Result<BTreeMap<String, String>, TaskError> {
        let mut child = crate::process::spawn(
            self.command()
                .args(["cat-file", "--batch"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped()),
        )?;
        let input: String = specs.iter().map(|spec| format!("{spec}\n")).collect();
        let mut stdin = child.stdin.take();
        let writer = std::thread::spawn(move || {
            stdin
                .as_mut()
                .map(|stdin| stdin.write_all(input.as_bytes()))
        });
        let output = child.wait_with_output()?;
        if let Ok(Some(Err(error))) = writer.join() {
            return Err(error.into());
        }
        if !output.status.success() {
            return Err(TaskError::Message("git cat-file --batch failed".into()));
        }
        diff::batch(specs, &output.stdout)
    }

    fn sources(
        &self,
        rev: &str,
        paths: impl Iterator<Item = String>,
    ) -> Result<BTreeMap<String, String>, TaskError> {
        let paths: Vec<_> = paths.collect();
        let specs: Vec<_> = paths.iter().map(|path| format!("{rev}:{path}")).collect();
        let blobs = self.blobs(&specs)?;
        Ok(paths
            .into_iter()
            .zip(specs)
            .filter_map(|(path, spec)| Some((path, blobs.get(&spec)?.clone())))
            .collect())
    }

    fn protected(&self, base: &str, head: &str) -> Result<Vec<String>, TaskError> {
        let mut protected = match fs::read_to_string(self.root.join(PROTECTED_LIST)) {
            Ok(text) => diff::patterns(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        let specs = [base, head].map(|rev| format!("{rev}:{PROTECTED_LIST}"));
        for text in self.blobs(&specs)?.values() {
            protected.extend(diff::patterns(text));
        }
        protected.sort();
        protected.dedup();
        if protected.is_empty() {
            return Err(TaskError::Message(format!(
                "guard: {PROTECTED_LIST} is absent at {base}, at {head} and in the checkout"
            )));
        }
        Ok(protected)
    }

    fn inputs(&self, base: &str, head: &str) -> Result<Inputs, TaskError> {
        let range = format!("{base}..{head}");
        let mut status = vec!["diff", "-z", "--name-status"];
        status.extend(DIFF_FLAGS);
        status.extend([base, head]);
        let changes = diff::name_status(&self.read(&status)?);
        let mut unified = vec!["diff", "-U0"];
        unified.extend(DIFF_FLAGS);
        unified.extend([base, head]);
        let lines = diff::unified(&self.read(&unified)?);
        if let Some(file) = rules::unparsed(&lines) {
            return Err(TaskError::Message(format!(
                "guard: git diff output not understood near {file:?}"
            )));
        }
        let log = [
            "log",
            "--no-renames",
            "--name-only",
            "-z",
            "--diff-merges=first-parent",
            diff::LOG_FORMAT,
            &range,
        ];
        let read = |path: &&String| path.ends_with(".rs") || diff::is_test_file(path);
        let olds = changes.iter().filter_map(|change| change.old.clone());
        let news = changes.iter().filter_map(|change| change.new.clone());
        Ok(Inputs {
            protected: self.protected(base, head)?,
            base_sources: self.sources(base, olds.filter(|path| read(&path)))?,
            head_sources: self.sources(head, news.filter(|path| read(&path)))?,
            base_crates: self.crates(base)?,
            head_crates: self.crates(head)?,
            commits: diff::commits(&self.read(&log)?),
            changes,
            lines,
        })
    }

    fn crates(&self, rev: &str) -> Result<Vec<String>, TaskError> {
        let files = self.read(&["ls-tree", "-r", "-z", "--name-only", rev])?;
        Ok(diff::crate_directories(files.split('\0')))
    }
}

#[cfg(test)]
mod tests;
