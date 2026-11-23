use super::*;
use crate::source::{DirSource, Kind, Source};
use crate::testing::{self, Result};
use pyload::format::{flags, testing as blobs};

#[test]
fn lexically_latest_resource_and_service_validation() -> Result<()> {
    let blob = blobs::blob(
        &[
            blobs::entry(
                b"awscli/botocore/data/s3/2006-03-01/endpoint-rule-set-1.json",
                b"old",
                flags::RESOURCE,
            ),
            blobs::entry(
                b"awscli/botocore/data/s3/2099-01-01/endpoint-rule-set-1.json",
                b"new",
                flags::RESOURCE,
            ),
            blobs::entry(
                b"awscli/botocore/data/s3/9999/other.json",
                b"ignore",
                flags::RESOURCE,
            ),
        ],
        false,
        2,
    );
    let view = View::new(&blob)?;
    let name = versioned_resource_name(view, "s3", "endpoint-rule-set-1")?;
    assert_eq!(
        name,
        b"awscli/botocore/data/s3/2099-01-01/endpoint-rule-set-1.json"
    );
    assert_eq!(load_resource(view, name)?, b"new");
    for service in ["", "s3/../ec2", "s3_", "s3."] {
        assert!(matches!(
            versioned_resource_name(view, service, "endpoint-rule-set-1"),
            Err(ResourceError::InvalidServiceName)
        ));
    }
    assert!(matches!(
        versioned_resource_name(view, "ec2", "endpoint-rule-set-1"),
        Err(ResourceError::Missing)
    ));
    Ok(())
}

#[test]
fn compressed_resources_decode_through_the_reserved_scratch() -> Result<()> {
    let rules = br#"{"version":"1.0"}"#;
    let frame = blobs::raw_zstd_frame(rules)?;
    let mut compressed = blobs::entry(
        b"awscli/botocore/data/s3/2006-03-01/endpoint-rule-set-1.json",
        &frame,
        flags::RESOURCE | flags::COMPRESSED,
    );
    compressed.raw_len = u32::try_from(rules.len())?;
    let mut corrupt = blobs::entry(
        b"awscli/botocore/data/s3/2006-03-01/endpoint-tests-1.json",
        &frame,
        flags::RESOURCE | flags::COMPRESSED,
    );
    corrupt.raw_len = compressed.raw_len + 1;
    let blob = blobs::blob(&[compressed, corrupt], false, 2);
    let view = View::new(&blob)?;
    let name = versioned_resource_name(view, "s3", "endpoint-rule-set-1")?;
    assert_eq!(load_resource(view, name)?, rules);
    assert!(matches!(
        load_resource(
            view,
            b"awscli/botocore/data/s3/2006-03-01/endpoint-tests-1.json"
        ),
        Err(ResourceError::Decode(
            pyload::decompress::Error::DecodeFailed
        ))
    ));
    Ok(())
}

/// `contains` finds an entry without decompressing it: a corrupt frame,
/// which a read fails on, is there all the same.
#[test]
fn contains_finds_an_entry_without_decompressing_it() -> Result<()> {
    let name = b"awscli/botocore/data/s3/2006-03-01/endpoint-rule-set-1.json";
    let mut corrupt = blobs::entry(
        name,
        b"not a zstd frame",
        flags::RESOURCE | flags::COMPRESSED,
    );
    corrupt.raw_len = 17;
    let blob = blobs::blob(&[corrupt], false, 2);
    let source = PayloadSource::botocore(View::new(&blob)?);
    assert!(source.read("s3/2006-03-01/endpoint-rule-set-1").is_err());
    assert!(source.contains("s3/2006-03-01/endpoint-rule-set-1")?);
    assert!(!source.contains("s3/2006-03-01/service-2")?);
    Ok(())
}

#[test]
fn compressed_decode_failures_keep_the_decoder_error_names() -> Result<()> {
    let rules = br#"{"version":"1.0"}"#;
    let frame = blobs::raw_zstd_frame(rules)?;
    let corrupt_name = b"awscli/botocore/data/s3/2006-03-01/endpoint-rule-set-1.json";
    let mut corrupt = blobs::entry(
        corrupt_name,
        b"not a zstd frame",
        flags::RESOURCE | flags::COMPRESSED,
    );
    corrupt.raw_len = u32::try_from(rules.len())?;
    let over_long_name = b"awscli/botocore/data/s3/2099-01-01/endpoint-rule-set-1.json";
    let mut over_long = blobs::entry(over_long_name, &frame, flags::RESOURCE | flags::COMPRESSED);
    over_long.raw_len = u32::try_from(rules.len() - 1)?;
    let blob = blobs::blob(&[corrupt, over_long], false, 2);
    let view = View::new(&blob)?;
    for (name, expected, text) in [
        (
            &corrupt_name[..],
            pyload::decompress::Error::DecodeFailed,
            "DecodeFailed",
        ),
        (
            &over_long_name[..],
            pyload::decompress::Error::OutputLengthMismatch,
            "OutputLengthMismatch",
        ),
    ] {
        let error = load_resource(view, name)
            .err()
            .ok_or("a corrupt resource loaded")?;
        assert!(
            matches!(&error, ResourceError::Decode(actual) if *actual == expected),
            "{error:?}"
        );
        assert_eq!(error.to_string(), text);
    }
    Ok(())
}

fn two_prefixes(model: &[u8]) -> Vec<u8> {
    let entries = [
        (
            b"awscli/botocore/data/a/2019-01-01/service-2.json".as_slice(),
            b"{}".as_slice(),
        ),
        (b"awscli/botocore/data/a/2020-01-01/service-2.json", model),
        (b"awscli/botocore/data/a-b/2020-01-01/service-2.json", b"{}"),
        (b"awscli/botocore/data/b/2020-01-01/waiters-2.json", b"{}"),
        (b"awscli/botocore/data/partitions.json", b"[]"),
        (b"awscli/data/cli.json", b"{}"),
        (b"awscli/data/z/2020-01-01/service-2.json", b"{}"),
    ];
    let entries = entries.map(|(name, data)| blobs::entry(name, data, flags::RESOURCE));
    blobs::blob(&entries, false, 2)
}

#[test]
fn payload_sources_list_their_own_prefix_in_sorted_order() -> Result<()> {
    let blob = two_prefixes(b"{}");
    let view = View::new(&blob)?;
    let botocore = PayloadSource::botocore(view);
    let mut checks = testing::Checks::default();
    checks.eq(
        "services",
        &botocore.services(Kind::Service)?[..],
        &["a", "a-b"][..],
    );
    checks.eq(
        "waiters",
        &botocore.services(Kind::Waiters)?[..],
        &["b"][..],
    );
    let versions = botocore.versions("a", Kind::Service)?;
    checks.eq("versions", &versions[..], &["2019-01-01", "2020-01-01"][..]);
    checks.is(
        "nested name",
        botocore.versions("a/2020-01-01", Kind::Service)?.is_empty(),
    );
    checks.is(
        "empty name",
        botocore.versions("", Kind::Service)?.is_empty(),
    );
    let awscli = PayloadSource::awscli(view);
    checks.eq(
        "awscli services",
        &awscli.services(Kind::Service)?[..],
        &["z"][..],
    );
    checks.finish()
}

#[test]
fn payload_sources_read_their_own_prefix() -> Result<()> {
    let model = testing::service_json("2020-01-01", &["Op"]);
    let blob = two_prefixes(&model);
    let view = View::new(&blob)?;
    let (botocore, awscli) = (PayloadSource::botocore(view), PayloadSource::awscli(view));
    let mut checks = testing::Checks::default();
    checks.eq(
        "model",
        &botocore.read("a/2020-01-01/service-2")?,
        &Some(model.clone()),
    );
    checks.eq(
        "partitions",
        &botocore.read("partitions")?,
        &Some(b"[]".to_vec()),
    );
    checks.eq("cli in botocore", &botocore.read("cli")?, &None);
    checks.eq("cli in awscli", &awscli.read("cli")?, &Some(b"{}".to_vec()));
    checks.eq("partitions in awscli", &awscli.read("partitions")?, &None);
    checks.finish()
}

#[test]
fn the_embedded_payload_holds_the_repository_data_directory() -> Result<()> {
    let payload = testing::payload()?;
    let view = View::new(&payload)?;
    let embedded = PayloadSource::botocore(view);
    let disk = DirSource::new(testing::data_dir());
    for kind in [
        Kind::Service,
        Kind::Paginators,
        Kind::Waiters,
        Kind::EndpointRuleSet,
    ] {
        assert_eq!(embedded.services(kind)?, disk.services(kind)?, "{kind:?}");
    }
    let directories = std::fs::read_dir(testing::data_dir())?
        .filter(|entry| entry.as_ref().is_ok_and(|entry| entry.path().is_dir()))
        .count();
    assert_eq!(embedded.services(Kind::Service)?.len(), directories);
    for service in ["sts", "ec2"] {
        let version = crate::source::latest_version(&embedded, service, Kind::Service)?;
        assert_eq!(
            version,
            crate::source::latest_version(&disk, service, Kind::Service)?
        );
        let name = format!("{service}/{version}/service-2");
        assert_eq!(embedded.read(&name)?, disk.read(&name)?);
    }
    Ok(())
}

/// Model load on the built binary's own payload, best of seven per stage:
/// the entry's zstd decode, the JSON parse, the arena, and `Model::load`
/// whole (which also merges any `sdk-extras`). The binary is
/// `AWS_NATIVE_TIMING_BINARY`; `tests/numbers/measure.py` sets it and runs
/// `RUST_TEST_NOCAPTURE=1 cargo xtask test -p model --profile release-safe
/// --filter --ignored`.
#[test]
#[ignore = "a timing report, not a check"]
fn payload_load_timings() -> Result<()> {
    let Some(binary) = std::env::var_os("AWS_NATIVE_TIMING_BINARY") else {
        return Err("AWS_NATIVE_TIMING_BINARY names no binary".into());
    };
    let bytes = std::fs::read(&binary)?;
    let located = pyload::locate(&bytes)?;
    let source = PayloadSource::botocore(pyload::View::new(located.bytes)?);
    let mut report =
        String::from("model-load service  json_bytes  decode_ms  parse_ms  arena_ms  load_ms\n");
    for service in ["sts", "dynamodb", "s3", "ec2"] {
        let version = crate::source::latest_version(&source, service, Kind::Service)?;
        let name = format!("{service}/{version}/service-2");
        let best = |times: &mut std::time::Duration, started: std::time::Instant| {
            *times = (*times).min(started.elapsed());
        };
        let max = std::time::Duration::MAX;
        let (mut decode, mut parse, mut arena, mut load) = (max, max, max, max);
        let mut size = 0;
        for _ in 0..7 {
            let started = std::time::Instant::now();
            let json = source.read(&name)?.ok_or("no service-2 entry")?;
            best(&mut decode, started);
            size = json.len();
            let started = std::time::Instant::now();
            let value: serde_json::Value = serde_json::from_slice(&json)?;
            best(&mut parse, started);
            let started = std::time::Instant::now();
            let model = crate::shapes::Model::from_value(&value)?;
            best(&mut arena, started);
            drop(model);
            let started = std::time::Instant::now();
            let model = crate::shapes::Model::load(&source, service)?;
            best(&mut load, started);
            drop(model);
        }
        let ms = |time: std::time::Duration| time.as_secs_f64() * 1e3;
        report.push_str(&format!(
            "model-load {service:<9} {size:>10} {:>10.2} {:>9.2} {:>9.2} {:>8.2}\n",
            ms(decode),
            ms(parse),
            ms(arena),
            ms(load)
        ));
    }
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    Ok(())
}
