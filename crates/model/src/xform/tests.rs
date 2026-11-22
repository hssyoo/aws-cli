use super::*;
use crate::source::{DirSource, Kind, Source, load_service_model};
use crate::testing::{self, Result};

fn disk_cases() -> Result<SpecialCases> {
    let source = std::fs::read_to_string(testing::botocore_dir().join("__init__.py"))?;
    Ok(SpecialCases::parse(&source)?)
}

/// Runs botocore's `__init__.py` alone, so only its `xform_name` and
/// `_xform_cache` are in play.
const ORACLE: &str = r#"
import importlib.util, json, sys
path, = sys.argv[1:]
spec = importlib.util.spec_from_file_location('botocore_init', path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
names = json.loads(sys.stdin.read())
special = len(module._xform_cache)
pairs = [[module.xform_name(name, '-'), module.xform_name(name, '_')] for name in names]
sys.stdout.write(json.dumps({'special': special, 'names': pairs}))
"#;

fn oracle(names: &[String]) -> Result<(usize, Vec<(String, String)>)> {
    let path = testing::botocore_dir().join("__init__.py");
    let script = ORACLE.replace(
        "path, = sys.argv[1:]",
        &format!("path = {:?}", path.display().to_string()),
    );
    let output = testing::python(&script, serde_json::to_string(names)?.as_bytes())?;
    let output: serde_json::Value = serde_json::from_slice(&output)?;
    let special = output["special"].as_u64().ok_or("special")?;
    let pairs = serde_json::from_value(output["names"].clone())?;
    Ok((usize::try_from(special)?, pairs))
}

fn operation_names() -> Result<Vec<String>> {
    let source = DirSource::new(testing::data_dir());
    let mut names = Vec::new();
    for service in source.services(Kind::Service)? {
        let document = load_service_model(&source, &service, Kind::Service)?;
        let operations = document.json["operations"]
            .as_object()
            .ok_or("operations")?;
        names.extend(operations.keys().cloned());
    }
    Ok(names)
}

#[test]
fn xform_name_agrees_with_botocore_for_every_operation_name() -> Result<()> {
    let mut names = operation_names()?;
    let operations = names.len();
    for edge in [
        "",
        "A",
        "ABCs",
        "ListARNs",
        "GetObjectACLs",
        "a-b",
        "a_b",
        "HTTPSConnection",
        "S3Bucket",
        "Get2FA",
        "IPv6Address",
        "ÄBc",
        "aÄBc",
        "line\nBreak",
        "XMLs",
        "ABs",
        "CreateCachediSCSIVolume",
        "createCachediSCSIVolume",
        "Aa",
        "AAa",
        "aAA",
    ] {
        names.push(edge.to_owned());
    }
    let (special_count, expected) = oracle(&names)?;
    let special = disk_cases()?;
    assert_eq!(special.len(), special_count);
    assert!(operations > 10_000, "only {operations} operation names");
    let mut differences = Vec::new();
    for (name, (hyphen, underscore)) in names.iter().zip(&expected) {
        let actual = (
            xform_name(name, "-", &special),
            xform_name(name, "_", &special),
        );
        if (&actual.0, &actual.1) != (hyphen, underscore) {
            differences.push(format!(
                "{name:?}: {actual:?} != ({hyphen:?}, {underscore:?})"
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    Ok(())
}

#[test]
fn the_compiled_in_special_cases_are_the_vendored_files() -> Result<()> {
    assert_eq!(SpecialCases::botocore()?, disk_cases()?);
    assert!(!SpecialCases::botocore()?.is_empty());
    Ok(())
}

#[test]
fn special_cases_apply_per_separator_and_a_separator_in_the_name_returns_it() -> Result<()> {
    let cases = SpecialCases::parse(concat!(
        "x = 1\n_xform_cache = {\n    ('OneThing', '_'): 'one_thing_x',\n",
        "    (\n        \"OneThing\",\n        '-',\n    ): \"one-thing-y\",  # note\n",
        "}\nrest = 2\n",
    ))?;
    assert_eq!(cases.len(), 2);
    assert_eq!(xform_name("OneThing", "_", &cases), "one_thing_x");
    assert_eq!(xform_name("OneThing", "-", &cases), "one-thing-y");
    assert_eq!(xform_name("OneThing", ".", &cases), "one.thing");
    assert_eq!(xform_name("One-Thing", "-", &cases), "One-Thing");
    assert_eq!(
        xform_name("DescribeDBInstances", "-", &SpecialCases::default()),
        "describe-db-instances"
    );
    Ok(())
}

#[test]
fn a_cache_python_could_not_have_written_is_refused() {
    for source in [
        "",
        "_xform_cache = {",
        "_xform_cache = {('A', '_'): 'a',",
        "_xform_cache = {('A', '_') 'a'}",
        "_xform_cache = {('A'): 'a'}",
        "_xform_cache = {('A', '_'): 'a' + 'b'}",
        "_xform_cache = {('A', '_'): 'a\\n'}",
        "_xform_cache = {('A', '_'): f'a'}",
    ] {
        assert!(SpecialCases::parse(source).is_err(), "{source:?}");
    }
}
