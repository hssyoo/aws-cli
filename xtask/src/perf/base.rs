//! The base binary a comparison measures the head against.
//!
//! It is `--base-binary PATH` as given, or a build of a commit: `--base
//! REV`, or by default the merge-base of `HEAD` with the main line. The
//! main line is never guessed. It is `--mainline REF`, else the git
//! setting `perf.mainline` (`git config perf.mainline REF` names it once
//! for a checkout, such as a branch in a clone with no remote), else the
//! `origin` remote's default branch, `refs/remotes/origin/HEAD`; with none
//! of them the check stops and says so.
//!
//! A commit is built from a detached export of its tree (`git archive`)
//! under `target/perf/base/<commit>/`, by that tree's own `cargo xtask
//! build`, into a target directory all base builds share, and the binary
//! is kept as `target/perf/base/<commit>/aws`: a later comparison with the
//! same base reuses it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::options::Options;
use crate::product::options::DEFAULT_PROFILE;
use crate::{TaskError, deps, process};

/// The base: its binary and, when it was built here, its commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Base {
    pub(super) binary: PathBuf,
    pub(super) commit: Option<String>,
}

/// The base `options` name, built if it is a commit not built before.
pub(super) fn resolve(root: &Path, options: &Options) -> Result<Base, TaskError> {
    if let Some(path) = &options.base_binary {
        let binary = root.join(path);
        if !binary.is_file() {
            return Err(deps::error(format!(
                "base binary is absent: {}",
                binary.display()
            )));
        }
        return Ok(Base {
            binary,
            commit: options.base_commit.clone(),
        });
    }
    let commit = match &options.base {
        Some(revision) => commit_of(root, revision)?,
        None => {
            let (mainline, source) = mainline(root, options)?;
            let base = git(root, &["merge-base", "HEAD", &mainline])?;
            writeln!(
                std::io::stdout(),
                "perf: base {base}, the merge-base of HEAD and the main line {mainline} \
                 ({source})"
            )?;
            base
        }
    };
    Ok(Base {
        binary: build(root, &commit)?,
        commit: Some(commit),
    })
}

/// The main line and where its name came from.
fn mainline(root: &Path, options: &Options) -> Result<(String, &'static str), TaskError> {
    if let Some(mainline) = &options.mainline {
        return Ok((mainline.clone(), "--mainline"));
    }
    if let Some(mainline) = optional_git(root, &["config", "--get", "perf.mainline"])? {
        return Ok((mainline, "git config perf.mainline"));
    }
    let remote = ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"];
    if let Some(mainline) = optional_git(root, &remote)? {
        return Ok((mainline, "the origin remote's default branch"));
    }
    Err(deps::error(
        "no main line to compare with: pass --base REV or --base-binary PATH, or name the \
         main line with --mainline REF or `git config perf.mainline REF`",
    ))
}

fn commit_of(root: &Path, revision: &str) -> Result<String, TaskError> {
    git(
        root,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )
}

fn git(root: &Path, args: &[&str]) -> Result<String, TaskError> {
    let output = process::output(Command::new("git").current_dir(root).args(args))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `git args`'s output, or `None` when it fails or prints nothing.
fn optional_git(root: &Path, args: &[&str]) -> Result<Option<String>, TaskError> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((output.status.success() && !text.is_empty()).then_some(text))
}

/// Where the base binary of `commit` is kept.
pub(super) fn cached(root: &Path, commit: &str) -> PathBuf {
    root.join("target/perf/base")
        .join(commit)
        .join("aws")
        .with_extension(std::env::consts::EXE_EXTENSION)
}

/// The binary of `commit`, built unless it was before.
fn build(root: &Path, commit: &str) -> Result<PathBuf, TaskError> {
    let binary = cached(root, commit);
    if binary.is_file() {
        writeln!(std::io::stdout(), "perf: base binary {}", binary.display())?;
        return Ok(binary);
    }
    let directory = root.join("target/perf/base").join(commit);
    let source = directory.join("src");
    if source.exists() {
        fs::remove_dir_all(&source)?;
    }
    fs::create_dir_all(&source)?;
    writeln!(
        std::io::stdout(),
        "perf: building the base {commit} in {}",
        source.display()
    )?;
    export(root, commit, &source)?;
    let target = root.join("target/perf/base/target");
    let mut command = Command::new("cargo");
    command
        .current_dir(&source)
        .args(["xtask", "build"])
        .env("CARGO_TARGET_DIR", &target);
    process::run(&mut command)?;
    let built = target
        .join(deps::TARGET)
        .join(DEFAULT_PROFILE)
        .join("aws")
        .with_extension(std::env::consts::EXE_EXTENSION);
    fs::copy(&built, &binary)?;
    fs::remove_dir_all(&source)?;
    Ok(binary)
}

/// Write the tree of `commit` into `source`: `git archive` into `tar`.
fn export(root: &Path, commit: &str, source: &Path) -> Result<(), TaskError> {
    let mut archive = process::spawn(
        Command::new("git")
            .current_dir(root)
            .args(["archive", "--format=tar", commit])
            .stdout(Stdio::piped()),
    )?;
    let stream = archive
        .stdout
        .take()
        .ok_or_else(|| deps::error("git archive gave no output stream"))?;
    let mut unpack = Command::new("tar");
    unpack.arg("-x").arg("-C").arg(source).stdin(stream);
    let unpacked = process::run(&mut unpack);
    let archived = archive.wait()?;
    unpacked?;
    if !archived.success() {
        return Err(deps::error(format!(
            "git archive {commit} failed with {archived}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
