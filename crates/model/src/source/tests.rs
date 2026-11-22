use super::*;
use crate::testing::{Result, Scratch, service_json};

fn layout() -> Result<Scratch> {
    let root = Scratch::new("dir-source")?;
    root.write("a/2020-01-01/service-2.json", &service_json("2020-01-01", &["Old"]))?;
    root.write("a/2021-01-01/paginators-1.json", b"{}")?;
    root.write("b/2019-01-01/waiters-2.json", b"{}")?;
    root.write("file/2020-01-01/service-2.json", b"{}")?;
    root.mkdir("c/2020-01-01/service-2.json")?;
    root.write("d/notes.txt", b"")?;
    root.write("e/2020-01-01/service-2.json.gz", b"")?;
    root.write("partitions.json", br#"{"partitions":[]}"#)?;
    Ok(root)
}

#[test]
fn dir_source_lists_services_and_versions_holding_a_model_file() -> Result<()> {
    let root = layout()?;
    let source = DirSource::new(root.path());
    assert_eq!(source.services(Kind::Service)?, ["a", "file"]);
    assert_eq!(source.services(Kind::Paginators)?, ["a"]);
    assert_eq!(source.services(Kind::Waiters)?, ["b"]);
    assert!(source.services(Kind::EndpointRuleSet)?.is_empty());
    assert_eq!(source.versions("a", Kind::Service)?, ["2020-01-01"]);
    assert_eq!(source.versions("a", Kind::Paginators)?, ["2021-01-01"]);
    assert!(source.versions("c", Kind::Service)?.is_empty());
    assert!(source.versions("missing", Kind::Service)?.is_empty());
    Ok(())
}

#[test]
fn dir_source_reads_files_by_botocore_data_path() -> Result<()> {
    let root = layout()?;
    let source = DirSource::new(root.path());
    let model = source.read("a/2020-01-01/service-2")?;
    assert_eq!(model, Some(service_json("2020-01-01", &["Old"])));
    assert_eq!(source.read("partitions")?, Some(br#"{"partitions":[]}"#.to_vec()));
    assert_eq!(source.read("a/2021-01-01/service-2")?, None);
    assert_eq!(source.read("c/2020-01-01/service-2")?, None);
    for name in ["", "a//2020-01-01/service-2", "../a/2020-01-01/service-2", "a/./x", "a\0b"] {
        assert!(matches!(source.read(name), Err(LoadError::InvalidName(_))), "{name:?}");
    }
    Ok(())
}

#[test]
fn a_missing_or_empty_root_names_no_directory() -> Result<()> {
    let root = Scratch::new("missing-root")?;
    for source in [DirSource::new(root.path().join("absent")), DirSource::new("")] {
        assert!(source.services(Kind::Service)?.is_empty());
        assert!(source.versions("a", Kind::Service)?.is_empty());
        assert_eq!(source.read("partitions")?, None);
    }
    Ok(())
}

#[test]
fn the_latest_version_is_the_greatest_name() -> Result<()> {
    let root = Scratch::new("latest")?;
    for version in ["2019-12-31", "2020-01-01", "2020-01-01b", "1999"] {
        root.write(&format!("a/{version}/service-2.json"), b"{}")?;
    }
    root.write("a/2099-01-01/waiters-2.json", b"{}")?;
    let source = DirSource::new(root.path());
    assert_eq!(latest_version(&source, "a", Kind::Service)?, "2020-01-01b");
    assert_eq!(latest_version(&source, "a", Kind::Waiters)?, "2099-01-01");
    assert!(matches!(
        latest_version(&source, "a", Kind::Paginators),
        Err(LoadError::DataNotFound(name)) if name == "a"
    ));
    Ok(())
}

#[test]
fn a_service_model_needs_a_listed_service_and_takes_its_extras() -> Result<()> {
    let root = Scratch::new("extras")?;
    root.write(
        "a/2020-01-01/service-2.json",
        br#"{"metadata":{"x":1,"m":{"k":1,"j":2},"z":{"deep":1}},"operations":{}}"#,
    )?;
    root.write(
        "a/2020-01-01/service-2.sdk-extras.json",
        br#"{"version":1,"merge":{"metadata":{"m":{"k":9,"n":3},"z":5,"y":[1]}}}"#,
    )?;
    root.write("b/2020-01-01/service-2.json", br#"{"metadata":{"x":1}}"#)?;
    root.write("b/2020-01-01/service-2.sdk-extras.json", br#"{"version":1}"#)?;
    let source = DirSource::new(root.path());
    let document = load_service_model(&source, "a", Kind::Service)?;
    assert_eq!(document.version, "2020-01-01");
    let merged = r#"{"metadata":{"x":1,"m":{"k":9,"j":2,"n":3},"z":5,"y":[1]},"operations":{}}"#;
    assert_eq!(serde_json::to_string(&document.json)?, merged);
    let document = load_service_model(&source, "b", Kind::Service)?;
    assert_eq!(serde_json::to_string(&document.json)?, r#"{"metadata":{"x":1}}"#);
    assert!(matches!(
        load_service_model(&source, "c", Kind::Service),
        Err(LoadError::UnknownService(name)) if name == "c"
    ));
    Ok(())
}

#[test]
fn raw_bytes_pass_through_unless_extras_merge() -> Result<()> {
    let root = Scratch::new("bytes")?;
    let rules = b"{ \"version\" : \"1.0\" }";
    root.write("a/2020-01-01/endpoint-rule-set-1.json", rules)?;
    root.write("b/2020-01-01/endpoint-rule-set-1.json", rules)?;
    root.write(
        "b/2020-01-01/endpoint-rule-set-1.sdk-extras.json",
        br#"{"merge":{"parameters":{}}}"#,
    )?;
    let source = DirSource::new(root.path());
    let (version, bytes) = load_service_bytes(&source, "a", Kind::EndpointRuleSet)?;
    assert_eq!((version.as_str(), bytes.as_slice()), ("2020-01-01", rules.as_slice()));
    let (_, bytes) = load_service_bytes(&source, "b", Kind::EndpointRuleSet)?;
    assert_eq!(bytes, br#"{"version":"1.0","parameters":{}}"#);
    assert_eq!(load_data_bytes(&source, "a/2020-01-01/endpoint-rule-set-1")?, rules);
    assert!(matches!(
        load_data_bytes(&source, "partitions"),
        Err(LoadError::DataNotFound(name)) if name == "partitions"
    ));
    Ok(())
}

#[test]
fn malformed_json_is_an_error_naming_the_file() -> Result<()> {
    let root = Scratch::new("malformed")?;
    root.write("a/2020-01-01/service-2.json", b"{")?;
    let source = DirSource::new(root.path());
    let error = load_service_model(&source, "a", Kind::Service)
        .err()
        .ok_or("malformed JSON loaded")?;
    assert!(matches!(&error, LoadError::Json { name, .. } if name == "a/2020-01-01/service-2"));
    Ok(())
}

#[test]
fn deep_merge_follows_botocore_utils_deep_merge() -> Result<()> {
    let mut base: serde_json::Value =
        serde_json::from_str(r#"{"a":{"b":1,"c":{"d":1}},"e":{"f":1},"g":1}"#)?;
    let extra: serde_json::Value =
        serde_json::from_str(r#"{"a":{"c":{"x":2},"b":{"y":3}},"e":4,"g":{"h":5},"i":6}"#)?;
    deep_merge(&mut base, &extra);
    let expected = r#"{"a":{"b":{"y":3},"c":{"d":1,"x":2}},"e":4,"g":{"h":5},"i":6}"#;
    assert_eq!(serde_json::to_string(&base)?, expected);
    Ok(())
}
