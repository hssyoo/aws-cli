use super::*;
use crate::source::{Kind, Source, latest_version, load_service_model};
use crate::testing::{Result, Scratch, service_json};
use awscli_argparse::unpack::{expand_user, expand_vars};
use pyload::format::{flags, testing as blobs};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

type Env = HashMap<String, OsString>;

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
        .collect()
}

/// `os.path.expanduser(os.path.expandvars(path))`, as both passes apply it.
fn expander(env: &Env) -> impl Fn(&[u8]) -> Option<Vec<u8>> + '_ {
    |path| expand_user(&expand_vars(path, env)?, env)
}

fn search<'a>(view: View<'a>, env: &Env) -> std::result::Result<SearchSource<'a>, SearchError> {
    let data_path = env.get("AWS_DATA_PATH").map(OsString::as_os_str);
    SearchSource::for_cli(view, data_path, &expander(env))
}

fn joined(entries: &[&str]) -> String {
    entries.join(&SEPARATOR.to_string())
}

/// A payload with no models (a payload cannot be empty).
fn empty_payload() -> Vec<u8> {
    blobs::blob(
        &[blobs::entry(
            b"awscli/data/cli.json",
            b"{}",
            flags::RESOURCE,
        )],
        false,
        2,
    )
}

fn directory(path: impl Into<PathBuf>) -> Location {
    Location::Directory(path.into())
}

#[test]
fn the_cli_searches_user_paths_awscli_data_home_models_then_botocore_data() -> Result<()> {
    let blob = empty_payload();
    let view = View::new(&blob)?;
    let data_path = joined(&["$D/one", "~/two"]);
    let env = env(&[
        ("AWS_DATA_PATH", &data_path),
        ("D", "/d"),
        ("HOME", "/home/u"),
    ]);
    let source = search(view, &env)?;
    assert_eq!(
        source.locations(),
        [
            directory("/d/one"),
            directory("/home/u/two"),
            Location::AwscliData,
            directory("/home/u/.aws/models"),
            Location::BotocoreData,
        ]
    );
    let source = search(view, &self::env(&[("HOME", "/home/u")]))?;
    assert_eq!(
        source.locations(),
        [
            Location::AwscliData,
            directory("/home/u/.aws/models"),
            Location::BotocoreData
        ]
    );
    Ok(())
}

#[test]
fn entries_are_split_and_expanded_twice_as_awscli_then_create_loader_do() -> Result<()> {
    let blob = empty_payload();
    let view = View::new(&blob)?;
    let two = joined(&["/p", "/q"]);
    let env = env(&[
        ("AWS_DATA_PATH", "$X"),
        ("X", "$Y"),
        ("Y", "/y"),
        ("HOME", "/h"),
    ]);
    assert_eq!(search(view, &env)?.locations()[0], directory("/y"));
    let env = self::env(&[("AWS_DATA_PATH", "$S"), ("S", &two), ("HOME", "/h")]);
    assert_eq!(
        search(view, &env)?.locations()[..2],
        [directory("/p"), directory("/q")]
    );
    Ok(())
}

#[test]
fn an_empty_entry_is_kept_and_names_no_directory() -> Result<()> {
    let user = Scratch::new("empty-entry")?;
    user.write(
        "u/2020-01-01/service-2.json",
        &service_json("2020-01-01", &["Op"]),
    )?;
    let blob = empty_payload();
    let view = View::new(&blob)?;
    let user_path = user.path().to_string_lossy().into_owned();
    let data_path = joined(&["", &user_path]);
    let env = env(&[("AWS_DATA_PATH", &data_path), ("HOME", "/nonexistent-home")]);
    let source = search(view, &env)?;
    assert_eq!(
        source.locations()[..2],
        [directory(""), directory(user.path())]
    );
    assert_eq!(source.services(Kind::Service)?, ["u"]);
    let source = search(view, &self::env(&[("AWS_DATA_PATH", ""), ("HOME", "/h")]))?;
    assert_eq!(source.locations()[0], directory(""));
    Ok(())
}

/// Two search locations, as a Python-route check of the loader's order
/// lays them out: `zver` has a newer version in the second location,
/// `zfirst` the same file in both.
fn two_locations() -> Result<(Scratch, Scratch, Scratch)> {
    let (a, b, home) = (
        Scratch::new("a")?,
        Scratch::new("b")?,
        Scratch::new("home")?,
    );
    a.write(
        "zver/2020-01-01/service-2.json",
        &service_json("2020-01-01", &["OldOp"]),
    )?;
    b.write(
        "zver/2021-01-01/service-2.json",
        &service_json("2021-01-01", &["NewOp"]),
    )?;
    a.write(
        "zfirst/2020-01-01/service-2.json",
        &service_json("2020-01-01", &["FromA"]),
    )?;
    b.write(
        "zfirst/2020-01-01/service-2.json",
        &service_json("2020-01-01", &["FromB"]),
    )?;
    let models = ".aws/models/zhome/2020-01-01/service-2.json";
    home.write(models, &service_json("2020-01-01", &["HomeOp"]))?;
    Ok((a, b, home))
}

fn payload_with(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let entries: Vec<_> = entries
        .iter()
        .map(|(name, data)| blobs::entry(name, data, flags::RESOURCE))
        .collect();
    blobs::blob(&entries, false, 2)
}

fn operations(source: &impl Source, service: &str) -> Result<Vec<String>> {
    let document = load_service_model(source, service, Kind::Service)?;
    let operations = document.json["operations"]
        .as_object()
        .ok_or("operations")?;
    Ok(operations.keys().cloned().collect())
}

#[test]
fn services_unite_the_greatest_version_wins_and_the_first_file_wins() -> Result<()> {
    let (a, b, home) = two_locations()?;
    let builtin = service_json("2019-01-01", &["Builtin"]);
    let blob = payload_with(&[
        (
            b"awscli/botocore/data/zver/2019-01-01/service-2.json",
            &builtin,
        ),
        (
            b"awscli/botocore/data/zpay/2019-01-01/service-2.json",
            &builtin,
        ),
    ]);
    let view = View::new(&blob)?;
    let (a_path, b_path) = (a.path().to_string_lossy(), b.path().to_string_lossy());
    let home_path = home.path().to_string_lossy();
    let a_first = joined(&[&a_path, &b_path]);
    let env = env(&[("AWS_DATA_PATH", &a_first), ("HOME", &home_path)]);
    let source = search(view, &env)?;
    assert_eq!(
        source.services(Kind::Service)?,
        ["zfirst", "zhome", "zpay", "zver"]
    );
    let versions = source.versions("zver", Kind::Service)?;
    assert_eq!(versions, ["2019-01-01", "2020-01-01", "2021-01-01"]);
    assert_eq!(
        latest_version(&source, "zver", Kind::Service)?,
        "2021-01-01"
    );
    assert_eq!(operations(&source, "zver")?, ["NewOp"]);
    assert_eq!(operations(&source, "zfirst")?, ["FromA"]);
    assert_eq!(operations(&source, "zhome")?, ["HomeOp"]);
    assert_eq!(operations(&source, "zpay")?, ["Builtin"]);
    let b_first = joined(&[&b_path, &a_path]);
    let env = self::env(&[("AWS_DATA_PATH", &b_first), ("HOME", &home_path)]);
    assert_eq!(operations(&search(view, &env)?, "zfirst")?, ["FromB"]);
    Ok(())
}

/// A model is the payload's only where no user location supplies its
/// latest version's file or that file's extras.
#[test]
fn a_model_is_packed_unless_a_user_location_supplies_its_files() -> Result<()> {
    let (a, b, home) = two_locations()?;
    let builtin = service_json("2019-01-01", &["Builtin"]);
    let blob = payload_with(&[
        (
            b"awscli/botocore/data/zver/2019-01-01/service-2.json",
            &builtin,
        ),
        (
            b"awscli/botocore/data/zpay/2019-01-01/service-2.json",
            &builtin,
        ),
        (
            b"awscli/botocore/data/zext/2019-01-01/service-2.json",
            &builtin,
        ),
    ]);
    a.write(
        "zext/2019-01-01/service-2.sdk-extras.json",
        br#"{"merge":{"added":1}}"#,
    )?;
    let view = View::new(&blob)?;
    let (a_path, b_path) = (a.path().to_string_lossy(), b.path().to_string_lossy());
    let home_path = home.path().to_string_lossy();
    let env = env(&[
        ("AWS_DATA_PATH", &joined(&[&a_path, &b_path])),
        ("HOME", &home_path),
    ]);
    let source = search(view, &env)?;
    assert!(source.packed("zpay")?);
    for user in ["zver", "zfirst", "zhome", "zext"] {
        assert!(!source.packed(user)?, "{user}");
    }
    let source = search(view, &self::env(&[("HOME", &home_path)]))?;
    assert!(source.packed("zver")?);
    Ok(())
}

#[test]
fn awscli_data_is_searched_after_user_paths_and_before_home_models() -> Result<()> {
    let (user, home) = (Scratch::new("user")?, Scratch::new("home")?);
    let from_payload = service_json("2020-01-01", &["FromAwscliData"]);
    let blob = payload_with(&[(b"awscli/data/q/2020-01-01/service-2.json", &from_payload)]);
    let view = View::new(&blob)?;
    let models = ".aws/models/q/2020-01-01/service-2.json";
    home.write(models, &service_json("2020-01-01", &["FromHome"]))?;
    let home_path = home.path().to_string_lossy();
    let env = env(&[("HOME", &home_path)]);
    assert_eq!(operations(&search(view, &env)?, "q")?, ["FromAwscliData"]);
    user.write(
        "q/2020-01-01/service-2.json",
        &service_json("2020-01-01", &["FromUser"]),
    )?;
    let user_path = user.path().to_string_lossy();
    let env = self::env(&[("AWS_DATA_PATH", &user_path), ("HOME", &home_path)]);
    assert_eq!(operations(&search(view, &env)?, "q")?, ["FromUser"]);
    Ok(())
}

#[test]
fn partitions_and_extras_follow_the_same_search() -> Result<()> {
    let user = Scratch::new("partitions")?;
    user.write("partitions.json", b"user")?;
    user.write(
        "zpay/2019-01-01/service-2.sdk-extras.json",
        br#"{"merge":{"added":1}}"#,
    )?;
    let builtin = service_json("2019-01-01", &["Builtin"]);
    let blob = payload_with(&[
        (b"awscli/botocore/data/partitions.json", b"builtin"),
        (
            b"awscli/botocore/data/zpay/2019-01-01/service-2.json",
            &builtin,
        ),
    ]);
    let view = View::new(&blob)?;
    assert_eq!(
        search(view, &env(&[("HOME", "/h")]))?.read("partitions")?,
        Some(b"builtin".to_vec())
    );
    let user_path = user.path().to_string_lossy();
    let env = env(&[("AWS_DATA_PATH", &user_path), ("HOME", "/h")]);
    let source = search(view, &env)?;
    assert_eq!(source.read("partitions")?, Some(b"user".to_vec()));
    let document = load_service_model(&source, "zpay", Kind::Service)?;
    assert_eq!(document.json["added"], 1);
    Ok(())
}

#[test]
fn a_missing_home_models_directory_is_skipped() -> Result<()> {
    let home = Scratch::new("bare-home")?;
    let builtin = service_json("2019-01-01", &["Builtin"]);
    let blob = payload_with(&[(
        b"awscli/botocore/data/zpay/2019-01-01/service-2.json",
        &builtin,
    )]);
    let view = View::new(&blob)?;
    let home_path = home.path().to_string_lossy();
    let source = search(view, &env(&[("HOME", &home_path)]))?;
    assert_eq!(source.services(Kind::Service)?, ["zpay"]);
    assert_eq!(operations(&source, "zpay")?, ["Builtin"]);
    Ok(())
}

/// `awscli/__init__.py:28-37` always sets `AWS_DATA_PATH` before botocore
/// resolves `data_path`, and the environment wins over the config file
/// (`configprovider.py:55`), so the CLI never reads a profile's `data_path`.
/// The inputs here are the environment alone, and a directory only a
/// profile names contributes nothing.
#[test]
fn a_profile_data_path_is_ignored_as_on_the_python_route() -> Result<()> {
    let (profile_models, files) = (Scratch::new("profile-models")?, Scratch::new("files")?);
    let model = service_json("2020-01-01", &["ProfOp"]);
    profile_models.write("zprof/2020-01-01/service-2.json", &model)?;
    let config = format!(
        "[default]\ndata_path = {}\n",
        profile_models.path().display()
    );
    files.write("config", config.as_bytes())?;
    let config_path = files.path().join("config").to_string_lossy().into_owned();
    let blob = empty_payload();
    let view = View::new(&blob)?;
    let env = env(&[
        ("AWS_CONFIG_FILE", &config_path),
        ("HOME", "/nonexistent-home"),
    ]);
    let source = search(view, &env)?;
    assert!(source.services(Kind::Service)?.is_empty());
    assert!(
        !source
            .locations()
            .contains(&directory(profile_models.path()))
    );
    Ok(())
}

#[test]
fn a_path_python_would_expand_differently_is_refused() -> Result<()> {
    let blob = empty_payload();
    let view = View::new(&blob)?;
    let env = env(&[("AWS_DATA_PATH", "~someone/models"), ("HOME", "/h")]);
    assert!(matches!(
        search(view, &env),
        Err(SearchError::Unexpandable(_))
    ));
    assert!(matches!(
        search(view, &self::env(&[])),
        Err(SearchError::Unexpandable(_))
    ));
    Ok(())
}
