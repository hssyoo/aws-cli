//! `cargo xtask guard --base <rev> [--head <rev>]`: the repository-generic
//! checks a change must pass so that it cannot weaken the oracles it is
//! judged by. Each violation is one line on stdout, with what to do about
//! it, and any violation fails the run. The rules ([`rules`]) are:
//!
//! - `protected`: a change to a path in `xtask/guard/protected.txt`, read
//!   from base, head and the running checkout alike, so a change cannot
//!   drop its own entry and a range from before the list is still checked.
//! - `suppression`: an added `#[ignore]`, `#[allow(..)]` or `#[expect(..)]`
//!   (or the inner `#![..]` forms) with no justification: a `//` comment on
//!   the same or the previous line, or a non-empty `reason = ".."` (for
//!   `ignore`, `= ".."`) in the attribute.
//! - `test-count`: fewer `#[test]` functions in a crate (the deepest
//!   directory holding a `Cargo.toml`) at head than at base.
//! - `deleted-test`: a deleted test file (`tests.rs`, `testing.rs`, or any
//!   file under a `tests` directory), unless a commit message in the range
//!   names its path or a `Retires:` trailer matches it.
//! - `assert`: a removed or changed line containing `assert` in a test file
//!   that survives at head, unless a `Retires:` or `Asserts-changed:`
//!   trailer matches the file. A line that reappears unchanged (trimmed) in
//!   the same file, as when it moves, does not count.
//!
//! Trailer values are paths or globs (`*` within a segment, `**` across
//! segments), separated by commas or spaces.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use crate::TaskError;

mod diff;
mod rules;

use diff::{Change, FileLines};

const PROTECTED_LIST: &str = "xtask/guard/protected.txt";
/// A `#[test]` attribute, alone or under a path (`#[tokio::test]`), at the
/// start of a line.
const TEST_ATTRIBUTE: &str = r"^[[:space:]]*#\[([A-Za-z_][A-Za-z0-9_]*::)*test\]";

/// What the rules read, gathered from git once.
#[derive(Debug, Default)]
pub struct Inputs {
    pub protected: Vec<String>,
    pub changes: Vec<Change>,
    pub lines: Vec<FileLines>,
    /// The head text of each `.rs` file whose added lines hold a suppression.
    pub head_sources: BTreeMap<String, String>,
    pub base_tests: BTreeMap<String, usize>,
    pub head_tests: BTreeMap<String, usize>,
    pub messages: Vec<String>,
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

struct Git<'a> {
    root: &'a Path,
}

impl Git<'_> {
    /// Runs git in the repository; `Ok(None)` when git exits 1 with no
    /// stderr (as `git grep` does on no match, and `git show` never does).
    fn read(&self, args: &[&str]) -> Result<Option<String>, TaskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.root)
            .args(["-c", "core.quotePath=false"])
            .args(args)
            .output()?;
        if output.status.success() {
            return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
        }
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            return Ok(None);
        }
        Err(TaskError::Message(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    fn must(&self, args: &[&str]) -> Result<String, TaskError> {
        self.read(args)?
            .ok_or_else(|| TaskError::Message(format!("git {} failed", args.join(" "))))
    }

    fn commit(&self, rev: &str) -> Result<String, TaskError> {
        let spec = format!("{rev}^{{commit}}");
        Ok(self
            .must(&["rev-parse", "--verify", "--quiet", &spec])?
            .trim()
            .to_owned())
    }

    /// A file's text at a commit, or `None` when it is absent there.
    fn show(&self, rev: &str, path: &str) -> Result<Option<String>, TaskError> {
        let spec = format!("{rev}:{path}");
        let exists = Command::new("git")
            .arg("-C")
            .arg(self.root)
            .args(["cat-file", "-e", &spec])
            .output()?
            .status
            .success();
        if !exists {
            return Ok(None);
        }
        self.must(&["show", &spec]).map(Some)
    }

    fn inputs(&self, base: &str, head: &str) -> Result<Inputs, TaskError> {
        let mut protected = match fs::read_to_string(self.root.join(PROTECTED_LIST)) {
            Ok(text) => diff::patterns(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        for rev in [base, head] {
            if let Some(text) = self.show(rev, PROTECTED_LIST)? {
                protected.extend(diff::patterns(&text));
            }
        }
        protected.sort();
        protected.dedup();
        if protected.is_empty() {
            return Err(TaskError::Message(format!(
                "guard: {PROTECTED_LIST} is absent at {base}, at {head} and in the checkout"
            )));
        }
        let range = format!("{base}..{head}");
        let changes = diff::name_status(&self.must(&[
            "diff",
            "-M",
            "-z",
            "--no-ext-diff",
            "--name-status",
            base,
            head,
        ])?);
        let lines = diff::unified(&self.must(&[
            "diff",
            "-M",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            base,
            head,
        ])?);
        let messages = self.must(&["log", "--format=%B%x00", &range])?;
        let mut inputs = Inputs {
            protected,
            changes,
            lines,
            messages: messages.split('\0').map(str::to_owned).collect(),
            base_tests: self.test_counts(base)?,
            head_tests: self.test_counts(head)?,
            ..Inputs::default()
        };
        for path in rules::suppression_files(&inputs.lines) {
            if let Some(text) = self.show(head, &path)? {
                inputs.head_sources.insert(path, text);
            }
        }
        Ok(inputs)
    }

    /// `#[test]` functions per crate directory at a commit.
    fn test_counts(&self, rev: &str) -> Result<BTreeMap<String, usize>, TaskError> {
        let files = self.must(&["ls-tree", "-r", "-z", "--name-only", rev])?;
        let crates = diff::crate_directories(files.split('\0'));
        let grep = self
            .read(&["grep", "-c", "-E", TEST_ATTRIBUTE, rev, "--", "*.rs"])?
            .unwrap_or_default();
        let prefix = format!("{rev}:");
        let counts = grep.lines().filter_map(|line| {
            let (path, count) = line.strip_prefix(&prefix)?.rsplit_once(':')?;
            Some((path.to_owned(), count.parse().ok()?))
        });
        Ok(diff::per_crate(&crates, counts))
    }
}

#[cfg(test)]
mod tests;
