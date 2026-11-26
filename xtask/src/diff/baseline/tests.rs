use std::path::PathBuf;

use super::*;
use crate::diff::options::{self, Route};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn scratch(name: &str) -> Result<PathBuf, TaskError> {
    let directory =
        std::env::temp_dir().join(format!("xtask-diff-baseline-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

const SHA: &str = "d56b358bb0000000000000000000000000000000";
const OTHER: &str = "17a368a340000000000000000000000000000000";

#[test]
fn a_cached_binary_is_reused_only_while_its_stamp_holds() -> Result<(), TaskError> {
    let directory = scratch("stamp")?;
    let binary = directory.join("aws");
    // No binary, no stamp.
    assert!(!cached(&directory, SHA)?);
    fs::write(&binary, b"\x7fELF one")?;
    assert!(!cached(&directory, SHA)?, "a binary without a stamp");
    fs::write(directory.join("aws.stamp"), stamp(SHA, &binary)?)?;
    assert!(cached(&directory, SHA)?);
    // Evasion: another commit's binary under this one's name.
    assert!(!cached(&directory, OTHER)?);
    // Evasion: the binary changed after it was stamped (a partial copy, a
    // rebuild by hand).
    fs::write(&binary, b"\x7fELF two")?;
    assert!(!cached(&directory, SHA)?);
    // Evasion: a stamp left without its binary.
    fs::write(directory.join("aws.stamp"), stamp(SHA, &binary)?)?;
    fs::remove_file(&binary)?;
    assert!(!cached(&directory, SHA)?);
    fs::remove_dir_all(&directory)?;
    Ok(())
}

#[test]
fn the_stamp_names_the_commit_and_the_bytes() -> Result<(), TaskError> {
    let directory = scratch("bytes")?;
    let binary = directory.join("aws");
    fs::write(&binary, b"abc")?;
    assert_eq!(
        stamp(SHA, &binary)?,
        format!(
            "commit {SHA}\nsha256 \
             ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n"
        )
    );
    fs::remove_dir_all(&directory)?;
    Ok(())
}

#[test]
fn a_value_is_a_file_or_a_commit() -> Result<(), TaskError> {
    let root = root();
    // A file is the baseline as it is, and nothing is built.
    let (label, binary) = resolve(&root, "Cargo.toml")?;
    assert_eq!(
        (label.as_str(), binary),
        ("Cargo.toml", root.join("Cargo.toml"))
    );
    let head = commit(&root, "HEAD")?;
    assert!(is_sha(&head), "{head}");
    // Evasion: an option or a range smuggled into git as a revision.
    for value in [
        "",
        "--all",
        "-h",
        "HEAD..HEAD",
        "no-such-revision-b3-3",
        "HEAD:Cargo.toml",
    ] {
        let error = commit(&root, value)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            error.ends_with("is neither a file nor a commit"),
            "{value}: {error}"
        );
    }
    // A directory is not a binary.
    assert!(commit(&root, "xtask").is_err());
    Ok(())
}

#[test]
fn the_build_runs_in_the_worktree_with_its_own_target_and_toolchain() {
    let command = build_command(Path::new("/b/src"), Path::new("/b/target"));
    assert_eq!(command.get_current_dir(), Some(Path::new("/b/src")));
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["xtask", "build"]);
    let envs: Vec<_> = command.get_envs().collect();
    assert!(envs.contains(&("CARGO_TARGET_DIR".as_ref(), Some("/b/target".as_ref()))));
    // Evasion: the xtask's own toolchain or package steering the build.
    for name in STEERING.iter().filter(|name| **name != "CARGO_TARGET_DIR") {
        assert!(envs.contains(&(name.as_ref(), None)), "{name}");
    }
}

#[test]
fn the_artifact_is_the_default_one() {
    let artifact = artifact(Path::new("/t"));
    assert!(artifact.starts_with(Path::new("/t").join(deps::TARGET).join(DEFAULT_PROFILE)));
    assert_eq!(artifact.file_stem().and_then(|s| s.to_str()), Some("aws"));
}

#[test]
fn baseline_sets_both_sides_on_one_route() -> Result<(), TaskError> {
    let directory = scratch("plan")?;
    let old = directory.join("old");
    let new = directory.join("new");
    fs::write(&old, b"")?;
    fs::write(&new, b"")?;
    let text = |values: &[&str]| values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
    let old_text = old.display().to_string();
    let new_text = new.display().to_string();
    for (route, expected) in [(None, Route::Native), (Some("python"), Route::Python)] {
        let mut args = text(&["--baseline", &old_text, "--aws-binary", &new_text]);
        if let Some(route) = route {
            args.extend(text(&["--route", route]));
        }
        let run = crate::diff::plan(&root(), &options::parse(&args)?)?;
        assert_eq!((run.a.binary.clone(), run.a.route), (old.clone(), expected));
        assert_eq!((run.b.binary.clone(), run.b.route), (new.clone(), expected));
        assert_eq!(run.mode(), "baseline");
    }
    fs::remove_dir_all(&directory)?;
    Ok(())
}
