//! `--baseline SHA|PATH`: the binary a commit builds, cached, as side A.
//!
//! A value holding a `/` is a path, the baseline as it is. Otherwise it names
//! a commit, whose binary is built once and kept in the user's cache
//! directory, `aws-cli-native/diff-baseline/<sha>/` under `$XDG_CACHE_HOME`
//! or `~/.cache` ([`home`]), outside every checkout:
//!
//! - `src/`, a detached worktree of the commit, made afresh for each build;
//! - `target/`, its own target directory (the managed dependency store stays
//!   shared);
//! - `aws`, the default artifact copied out, and `aws.stamp`: the commit,
//!   the build recipe ([`RECIPE`]) and the copy's SHA-256.
//!
//! The build sees only the commit's own Cargo configuration: no directory
//! above `src` may hold a `.cargo/config` other than `$CARGO_HOME`'s
//! ([`foreign_configs`]), and `cargo xtask build` runs in a cleared
//! environment that keeps only [`KEPT`], so neither the candidate's
//! `.cargo/config.toml` nor the `[env]` its `cargo run` gave this process
//! reaches it. After the build the worktree must still be clean.
//!
//! A later run reuses `aws` while its stamp holds for the commit, the
//! recipe and the file's bytes, and builds again otherwise. A lock in the
//! directory keeps two runs from building one commit at once.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use super::sigv4::hex;
use crate::file_lock::FileLock;
use crate::product::options::DEFAULT_PROFILE;
use crate::{TaskError, deps, process};

/// The build recipe's version, in each stamp: bump it whenever
/// [`build_command`], [`KEPT`] or the artifact chosen changes, so no binary
/// built the old way is reused.
pub const RECIPE: u32 = 2;

/// The variables the baseline build keeps; every other one is cleared.
pub const KEPT: [&str; 22] = [
    "HOME",
    "PATH",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "XDG_CACHE_HOME",
    "AWS_NATIVE_CACHE",
    "AWS_NATIVE_CACHE_DIR",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// The baselines' directory: `aws-cli-native/diff-baseline` under
/// `$XDG_CACHE_HOME` (if absolute), else `~/.cache`.
pub fn home(variable: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, TaskError> {
    let absolute = |name| {
        variable(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let cache = absolute("XDG_CACHE_HOME")
        .or_else(|| Some(absolute("HOME")?.join(".cache")))
        .ok_or_else(|| deps::error("--baseline needs an absolute HOME or XDG_CACHE_HOME"))?;
    Ok(cache.join("aws-cli-native").join("diff-baseline"))
}

/// The baseline `value` names: its label and binary.
pub fn resolve(root: &Path, value: &str) -> Result<(String, PathBuf), TaskError> {
    if value.contains('/') {
        let path = root.join(value);
        if !path.is_file() {
            return Err(deps::error(format!(
                "--baseline: no file at {}",
                path.display()
            )));
        }
        return Ok((value.to_owned(), path));
    }
    let sha = commit(root, value)?;
    let directory = home(|name| std::env::var_os(name))?.join(&sha);
    let binary = binary(root, &directory, &sha)?;
    Ok((format!("baseline@{}", &sha[..12]), binary))
}

/// The full SHA of the commit `value` names.
fn commit(root: &Path, value: &str) -> Result<String, TaskError> {
    let not = || {
        deps::error(format!(
            "--baseline {value:?} is neither a path (holding a /) nor a commit"
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

/// The cached binary of commit `sha` in `directory`, built first unless its
/// stamp holds.
fn binary(root: &Path, directory: &Path, sha: &str) -> Result<PathBuf, TaskError> {
    fs::create_dir_all(directory)?;
    let _lock = FileLock::lock(&directory.join("lock"))?;
    let binary = directory.join("aws");
    let mut stdout = io::stdout().lock();
    if cached(directory, sha)? {
        writeln!(
            stdout,
            "diff: baseline {sha} reused: {} (its stamp holds)",
            binary.display()
        )?;
        return Ok(binary);
    }
    let source = directory.join("src");
    worktree(root, &source, sha)?;
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".cargo")));
    let foreign = foreign_configs(&source, cargo_home);
    if let Some(config) = foreign.first() {
        return Err(deps::error(format!(
            "the baseline build in {} would read {}, which is not the commit's; \
             set XDG_CACHE_HOME to a directory outside any Cargo project",
            source.display(),
            config.display()
        )));
    }
    writeln!(
        stdout,
        "diff: building baseline {sha} in {}",
        source.display()
    )?;
    drop(stdout);
    let target = directory.join("target");
    process::run(&mut build_command(
        Path::new("cargo"),
        &source,
        &target,
        std::env::vars_os(),
    ))?;
    clean(&source, sha, "after its build")?;
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

/// The Cargo configuration files in the directories above `source` (not
/// `source`'s own, the commit's), but `cargo_home`'s, which Cargo reads for
/// every build alike.
pub fn foreign_configs(source: &Path, cargo_home: Option<PathBuf>) -> Vec<PathBuf> {
    let home = cargo_home.and_then(|home| fs::canonicalize(home).ok());
    let start = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    start
        .ancestors()
        .skip(1)
        .map(|directory| directory.join(".cargo"))
        .filter(|cargo| home.is_none() || fs::canonicalize(cargo).ok() != home)
        .flat_map(|cargo| [cargo.join("config.toml"), cargo.join("config")])
        .filter(|config| config.is_file())
        .collect()
}

/// `program xtask build` in `source`, into `target`, every variable of
/// `environment` (this process's) but [`KEPT`] removed. The kept ones are
/// inherited, not set, so HOME stays as `process` requires.
fn build_command(
    program: &Path,
    source: &Path,
    target: &Path,
    environment: impl Iterator<Item = (OsString, OsString)>,
) -> Command {
    let mut build = Command::new(program);
    build.current_dir(source).args(["xtask", "build"]);
    for (name, _) in environment {
        if !KEPT.iter().any(|kept| name == *kept) {
            build.env_remove(name);
        }
    }
    build.env("CARGO_TARGET_DIR", target);
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

/// The stamp of `binary` built from `sha` by this recipe.
pub fn stamp(sha: &str, binary: &Path) -> Result<String, TaskError> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(binary)?, &mut hasher)?;
    Ok(format!(
        "commit {sha}\nrecipe {RECIPE} {} {DEFAULT_PROFILE}\nsha256 {}\n",
        deps::TARGET,
        hex(&hasher.finalize())
    ))
}

/// Makes `source` a fresh detached worktree at `sha`: one left by an earlier
/// build is removed first, so no file it or anyone left there reaches this
/// build. A `source` that is not a worktree of its own is never removed.
fn worktree(root: &Path, source: &Path, sha: &str) -> Result<(), TaskError> {
    if source.exists() {
        if !is_own_worktree(source)? {
            return Err(deps::error(format!(
                "{} is not a git worktree of its own; remove it and run again",
                source.display()
            )));
        }
        process::output(
            git(root)
                .args(["worktree", "remove", "--force"])
                .arg(source),
        )?;
    }
    // `--force` takes over a worktree registered at this path whose
    // directory is gone.
    let mut add = git(root);
    add.args(["worktree", "add", "--force", "--detach"])
        .arg(source)
        .arg(sha);
    process::output(&mut add)?;
    clean(source, sha, "before its build")
}

/// Whether `source` is the top of a git worktree, not a directory inside
/// another one.
fn is_own_worktree(source: &Path) -> Result<bool, TaskError> {
    let output = git(source)
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    let top = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(output.status.success() && fs::canonicalize(&top).ok() == fs::canonicalize(source).ok())
}

/// Fails unless `source` is its own worktree at `sha` with no tracked file
/// changed.
fn clean(source: &Path, sha: &str, when: &str) -> Result<(), TaskError> {
    let head = process::output(git(source).args(["rev-parse", "HEAD"]))?;
    let status =
        process::output(git(source).args(["status", "--porcelain", "--untracked-files=no"]))?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    if !is_own_worktree(source)? || head != sha || !status.stdout.is_empty() {
        return Err(deps::error(format!(
            "{} is not a clean worktree of {sha} {when} (HEAD {head}):\n{}",
            source.display(),
            String::from_utf8_lossy(&status.stdout)
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
