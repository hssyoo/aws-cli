//! `--baseline SHA|PATH`: the binary a commit builds, cached, as side A.
//!
//! A path to a file is the baseline as it is. Otherwise the value names a
//! commit, and its binary is built once and kept under
//! `target/diff/baseline/<sha>/`:
//!
//! - `src/`, a detached worktree of the commit;
//! - `target/`, its own target directory (the managed dependency store,
//!   `~/.cache`, stays shared);
//! - `aws`, the default artifact copied out, and `aws.stamp`, the commit and
//!   the copy's SHA-256.
//!
//! A later run reuses `aws` while its stamp holds for the commit and the
//! file's bytes, and builds again otherwise. A worktree at another commit, or
//! with tracked files changed, is never built from: the run fails and names
//! it. A lock in the directory keeps two runs from building one commit at once.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use super::sigv4::hex;
use crate::file_lock::FileLock;
use crate::product::options::DEFAULT_PROFILE;
use crate::{TaskError, deps, process};

/// Where the baselines live, under the repository root.
pub const DIRECTORY: &str = "target/diff/baseline";

/// The variables Cargo and rustup give the running xtask, which would
/// otherwise steer the baseline's own build: its toolchain, target
/// directory and package.
const STEERING: [&str; 9] = [
    "RUSTUP_TOOLCHAIN",
    "CARGO",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_MANIFEST_DIR",
    "CARGO_MANIFEST_PATH",
    "CARGO_CRATE_NAME",
    "CARGO_BIN_NAME",
    "CARGO_PRIMARY_PACKAGE",
];

/// The baseline `value` names: its label and binary.
pub fn resolve(root: &Path, value: &str) -> Result<(String, PathBuf), TaskError> {
    let path = root.join(value);
    if path.is_file() {
        return Ok((value.to_owned(), path));
    }
    let sha = commit(root, value)?;
    let binary = binary(root, &sha)?;
    Ok((format!("baseline@{}", &sha[..12]), binary))
}

/// The full SHA of the commit `value` names.
fn commit(root: &Path, value: &str) -> Result<String, TaskError> {
    let not = || {
        deps::error(format!(
            "--baseline {value:?} is neither a file nor a commit"
        ))
    };
    if value.is_empty() || value.starts_with('-') {
        return Err(not());
    }
    let mut command = git(root);
    command
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{value}^{{commit}}"));
    let output = command.output()?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !is_sha(&sha) {
        return Err(not());
    }
    Ok(sha)
}

fn is_sha(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn git(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory);
    command
}

/// The cached binary of commit `sha`, built first unless its stamp holds.
fn binary(root: &Path, sha: &str) -> Result<PathBuf, TaskError> {
    let directory = root.join(DIRECTORY).join(sha);
    fs::create_dir_all(&directory)?;
    let _lock = FileLock::lock(&directory.join("lock"))?;
    let binary = directory.join("aws");
    let mut stdout = io::stdout().lock();
    if cached(&directory, sha)? {
        writeln!(
            stdout,
            "diff: baseline {sha} reused: {} (its stamp holds)",
            binary.display()
        )?;
        return Ok(binary);
    }
    let source = directory.join("src");
    worktree(root, &source, sha)?;
    writeln!(
        stdout,
        "diff: building baseline {sha} in {}",
        source.display()
    )?;
    drop(stdout);
    let target = directory.join("target");
    process::run(&mut build_command(&source, &target))?;
    let artifact = artifact(&target);
    if !artifact.is_file() {
        return Err(deps::error(format!(
            "the baseline build left no binary at {}",
            artifact.display()
        )));
    }
    // A run that stops part way leaves no stamp, so the next one builds.
    remove(&directory.join("aws.stamp"))?;
    let partial = directory.join("aws.partial");
    fs::copy(&artifact, &partial)?;
    fs::rename(&partial, &binary)?;
    fs::write(directory.join("aws.stamp"), stamp(sha, &binary)?)?;
    Ok(binary)
}

/// `cargo xtask build` in `source`, into `target`.
fn build_command(source: &Path, target: &Path) -> Command {
    let mut build = Command::new("cargo");
    build
        .current_dir(source)
        .args(["xtask", "build"])
        .env("CARGO_TARGET_DIR", target);
    for name in STEERING.iter().filter(|name| **name != "CARGO_TARGET_DIR") {
        build.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CARGO_PKG_") {
            build.env_remove(name);
        }
    }
    build
}

/// The default artifact under a target directory, as `cargo xtask build`
/// writes it.
pub fn artifact(target: &Path) -> PathBuf {
    target
        .join(deps::TARGET)
        .join(DEFAULT_PROFILE)
        .join("aws")
        .with_extension(std::env::consts::EXE_EXTENSION)
}

/// Whether `directory`'s binary is the one its stamp records for `sha`.
pub fn cached(directory: &Path, sha: &str) -> Result<bool, TaskError> {
    let binary = directory.join("aws");
    let Ok(recorded) = fs::read_to_string(directory.join("aws.stamp")) else {
        return Ok(false);
    };
    Ok(binary.is_file() && recorded == stamp(sha, &binary)?)
}

/// The stamp of `binary` built from `sha`.
pub fn stamp(sha: &str, binary: &Path) -> Result<String, TaskError> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(binary)?, &mut hasher)?;
    Ok(format!(
        "commit {sha}\nsha256 {}\n",
        hex(&hasher.finalize())
    ))
}

/// Makes `source` a detached worktree at `sha`, or checks the one there is.
fn worktree(root: &Path, source: &Path, sha: &str) -> Result<(), TaskError> {
    if !source.exists() {
        // `--force` takes over a worktree registered at this path whose
        // directory is gone.
        let mut add = git(root);
        add.args(["worktree", "add", "--force", "--detach"])
            .arg(source)
            .arg(sha);
        process::output(&mut add)?;
    }
    let head = process::output(git(source).args(["rev-parse", "HEAD"]))?;
    let status =
        process::output(git(source).args(["status", "--porcelain", "--untracked-files=no"]))?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    if head != sha || !status.stdout.is_empty() {
        return Err(deps::error(format!(
            "{} is not a clean worktree of {sha} (HEAD {head}); remove it with \
             `git worktree remove --force {}` and run again",
            source.display(),
            source.display()
        )));
    }
    Ok(())
}

fn remove(path: &Path) -> Result<(), TaskError> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
