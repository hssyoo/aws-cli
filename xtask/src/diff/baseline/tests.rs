use std::ffi::OsString;
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
fn the_stamp_names_the_commit_the_recipe_and_the_bytes() -> Result<(), TaskError> {
    let directory = scratch("bytes")?;
    let binary = directory.join("aws");
    fs::write(&binary, b"abc")?;
    let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert_eq!(
        stamp(SHA, &binary)?,
        format!(
            "commit {SHA}\nrecipe {RECIPE} {} {DEFAULT_PROFILE}\nsha256 {hash}\n",
            deps::TARGET
        )
    );
    // rev-b33 S1. Evasion: a binary stamped by an earlier recipe (here the
    // first one, before the build left the candidate's tree) is reused.
    fs::write(
        directory.join("aws.stamp"),
        format!("commit {SHA}\nsha256 {hash}\n"),
    )?;
    assert!(!cached(&directory, SHA)?);
    let earlier = stamp(SHA, &binary)?.replace(
        &format!("recipe {RECIPE} "),
        &format!("recipe {} ", RECIPE - 1),
    );
    fs::write(directory.join("aws.stamp"), earlier)?;
    assert!(!cached(&directory, SHA)?);
    fs::remove_dir_all(&directory)?;
    Ok(())
}

#[test]
fn a_value_is_a_path_or_a_commit() -> Result<(), TaskError> {
    let root = root();
    // A path is the baseline as it is, and nothing is built.
    let (label, binary) = resolve(&root, "./Cargo.toml")?;
    assert_eq!(
        (label.as_str(), binary),
        ("./Cargo.toml", root.join("./Cargo.toml"))
    );
    // A path must hold a file.
    assert!(resolve(&root, "./xtask").is_err());
    assert!(resolve(&root, "./no-such-file").is_err());
    // A root file named like a ref does not shadow the ref: no `/`, a commit.
    let error = resolve(&root, "Cargo.toml")
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(error.ends_with("nor a commit"), "{error}");
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
        assert!(error.ends_with("nor a commit"), "{value}: {error}");
    }
    Ok(())
}

#[test]
fn the_baselines_live_in_the_users_cache() -> Result<(), TaskError> {
    let variable = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    };
    assert_eq!(
        home(variable(&[("HOME", "/h")]))?,
        PathBuf::from("/h/.cache/aws-cli-native/diff-baseline")
    );
    assert_eq!(
        home(variable(&[("HOME", "/h"), ("XDG_CACHE_HOME", "/x")]))?,
        PathBuf::from("/x/aws-cli-native/diff-baseline")
    );
    // A relative XDG_CACHE_HOME is ignored, as the XDG specification says.
    assert_eq!(
        home(variable(&[("HOME", "/h"), ("XDG_CACHE_HOME", "x")]))?,
        PathBuf::from("/h/.cache/aws-cli-native/diff-baseline")
    );
    assert!(home(variable(&[])).is_err());
    Ok(())
}

/// rev-b33 M1, as reproduced: a baseline worktree under the candidate's
/// tree inherits the candidate's `.cargo/config.toml` (its `[env]`, its
/// profiles), so a configuration change reaches both sides unseen.
#[test]
fn a_cargo_config_above_the_baseline_is_refused() -> Result<(), TaskError> {
    let outer = scratch("config")?;
    fs::create_dir_all(outer.join(".cargo"))?;
    fs::write(
        outer.join(".cargo/config.toml"),
        "[env]\nLEAK = \"from-candidate\"\n[profile.release]\noverflow-checks = true\n",
    )?;
    let source = outer.join("target/diff/baseline/sha/src");
    fs::create_dir_all(source.join(".cargo"))?;
    // The commit's own configuration is its own, and allowed.
    fs::write(
        source.join(".cargo/config.toml"),
        "[alias]\nxtask = \"run\"\n",
    )?;
    let found = foreign_configs(&source, None);
    let outer_config = fs::canonicalize(outer.join(".cargo/config.toml"))?;
    assert!(found.contains(&outer_config), "{found:?}");
    let own = fs::canonicalize(&source)?;
    assert!(
        !found.iter().any(|config| config.starts_with(&own)),
        "{found:?}"
    );
    // `$CARGO_HOME`'s configuration is read by every build alike.
    assert!(foreign_configs(&source, Some(outer.join(".cargo"))).is_empty());
    // A directory outside any project has none.
    let apart = scratch("apart")?.join("diff-baseline/sha/src");
    fs::create_dir_all(&apart)?;
    assert_eq!(foreign_configs(&apart, None), Vec::<PathBuf>::new());
    fs::remove_dir_all(&outer)?;
    Ok(())
}

#[test]
fn the_build_removes_every_variable_but_the_listed_ones() {
    // Evasion: the candidate's `[env]` (applied to this process by `cargo
    // run`), the toolchain rustup chose for it, or a user's flags reaching
    // the baseline's build.
    let inherited = [
        "HOME",
        "PATH",
        "LEAK",
        "RUSTUP_TOOLCHAIN",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_SAFE_OVERFLOW_CHECKS",
        "RUSTC_WRAPPER",
        "CARGO_TARGET_DIR",
        "CARGO_MANIFEST_DIR",
    ];
    let environment = inherited
        .iter()
        .map(|name| (OsString::from(name), OsString::from("x")));
    let command = build_command(
        Path::new("cargo"),
        Path::new("/b/src"),
        Path::new("/b/target"),
        environment,
    );
    assert_eq!(command.get_current_dir(), Some(Path::new("/b/src")));
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["xtask", "build"]);
    let mut envs: Vec<(String, Option<String>)> = command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    envs.sort();
    let mut expected: Vec<(String, Option<String>)> = inherited[2..]
        .iter()
        .filter(|name| **name != "CARGO_TARGET_DIR")
        .map(|name| ((*name).to_owned(), None))
        .chain([("CARGO_TARGET_DIR".to_owned(), Some("/b/target".to_owned()))])
        .collect();
    expected.sort();
    assert_eq!(envs, expected);
}

/// What `cargo test` gave this process (its `CARGO_*`, rustup's
/// `RUSTUP_TOOLCHAIN`) must not reach a stand-in for cargo.
#[cfg(unix)]
#[test]
fn the_build_sees_only_the_listed_variables() -> Result<(), TaskError> {
    use std::os::unix::fs::PermissionsExt;
    let directory = scratch("env")?;
    let program = directory.join("cargo");
    fs::write(&program, "#!/bin/sh\n/usr/bin/env\n")?;
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))?;
    let output = build_command(
        &program,
        &directory,
        Path::new("/b/target"),
        std::env::vars_os(),
    )
    .output()?;
    fs::remove_dir_all(&directory)?;
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    let shell = ["PWD", "SHLVL", "_", "OLDPWD", "CARGO_TARGET_DIR"];
    let strays: Vec<&str> = printed
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .filter(|name| !KEPT.contains(name) && !shell.contains(name))
        .collect();
    assert_eq!(strays, Vec::<&str>::new(), "{printed}");
    assert!(
        printed
            .lines()
            .any(|line| line == "CARGO_TARGET_DIR=/b/target")
    );
    Ok(())
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
    // rev-b33 N5: the candidate as its own baseline is refused, not run as
    // determinism under the baseline's name.
    let same = text(&["--baseline", &new_text, "--aws-binary", &new_text]);
    assert!(crate::diff::plan(&root(), &options::parse(&same)?).is_err());
    fs::remove_dir_all(&directory)?;
    Ok(())
}
