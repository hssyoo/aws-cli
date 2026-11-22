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
    assert_eq!(name, b"awscli/botocore/data/s3/2099-01-01/endpoint-rule-set-1.json");
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
        load_resource(view, b"awscli/botocore/data/s3/2006-03-01/endpoint-tests-1.json"),
        Err(ResourceError::Decode(pyload::decompress::Error::DecodeFailed))
    ));
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
        (&corrupt_name[..], pyload::decompress::Error::DecodeFailed, "DecodeFailed"),
        (
            &over_long_name[..],
            pyload::decompress::Error::OutputLengthMismatch,
            "OutputLengthMismatch",
        ),
    ] {
        let error = load_resource(view, name).err().ok_or("a corrupt resource loaded")?;
        assert!(matches!(&error, ResourceError::Decode(actual) if *actual == expected), "{error:?}");
        assert_eq!(error.to_string(), text);
    }
    Ok(())
}

#[test]
fn payload_sources_list_and_read_their_own_prefix() -> Result<()> {
    let model = testing::service_json("2020-01-01", &["Op"]);
    let blob = blobs::blob(
        &[
            blobs::entry(b"awscli/botocore/data/a/2019-01-01/service-2.json", b"{}", flags::RESOURCE),
            blobs::entry(b"awscli/botocore/data/a/2020-01-01/service-2.json", &model, flags::RESOURCE),
            blobs::entry(b"awscli/botocore/data/b/2020-01-01/waiters-2.json", b"{}", flags::RESOURCE),
            blobs::entry(b"awscli/botocore/data/partitions.json", b"[]", flags::RESOURCE),
            blobs::entry(b"awscli/data/cli.json", b"{}", flags::RESOURCE),
            blobs::entry(b"awscli/data/z/2020-01-01/service-2.json", b"{}", flags::RESOURCE),
        ],
        false,
        2,
    );
    let view = View::new(&blob)?;
    let botocore = PayloadSource::botocore(view);
    assert_eq!(botocore.services(Kind::Service)?, ["a"]);
    assert_eq!(botocore.services(Kind::Waiters)?, ["b"]);
    assert_eq!(botocore.versions("a", Kind::Service)?, ["2019-01-01", "2020-01-01"]);
    assert!(botocore.versions("a/2020-01-01", Kind::Service)?.is_empty());
    assert!(botocore.versions("", Kind::Service)?.is_empty());
    assert_eq!(botocore.read("a/2020-01-01/service-2")?, Some(model));
    assert_eq!(botocore.read("partitions")?, Some(b"[]".to_vec()));
    assert_eq!(botocore.read("cli")?, None);
    let awscli = PayloadSource::awscli(view);
    assert_eq!(awscli.services(Kind::Service)?, ["z"]);
    assert_eq!(awscli.read("cli")?, Some(b"{}".to_vec()));
    assert_eq!(awscli.read("partitions")?, None);
    Ok(())
}

#[test]
fn the_embedded_payload_holds_the_repository_data_directory() -> Result<()> {
    let payload = testing::payload()?;
    let view = View::new(&payload)?;
    let embedded = PayloadSource::botocore(view);
    let disk = DirSource::new(testing::data_dir());
    for kind in [Kind::Service, Kind::Paginators, Kind::Waiters, Kind::EndpointRuleSet] {
        assert_eq!(embedded.services(kind)?, disk.services(kind)?, "{kind:?}");
    }
    let directories = std::fs::read_dir(testing::data_dir())?
        .filter(|entry| entry.as_ref().is_ok_and(|entry| entry.path().is_dir()))
        .count();
    assert_eq!(embedded.services(Kind::Service)?.len(), directories);
    for service in ["sts", "ec2"] {
        let version = crate::source::latest_version(&embedded, service, Kind::Service)?;
        assert_eq!(version, crate::source::latest_version(&disk, service, Kind::Service)?);
        let name = format!("{service}/{version}/service-2");
        assert_eq!(embedded.read(&name)?, disk.read(&name)?);
    }
    Ok(())
}
