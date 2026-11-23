use super::*;
use pyload::format::{flags, testing};
/// The loader itself moved to `model::payload` with its tests; this checks
/// that `Rules::load` keeps reporting its failures as `RulesError`.
#[test]
fn rule_loading_failures_keep_their_rules_errors() -> Result<(), Box<dyn std::error::Error>> {
    let rules = br#"{"version":"1.0"}"#;
    let frame = testing::raw_zstd_frame(rules)?;
    let name = b"awscli/botocore/data/s3/2099-01-01/endpoint-rule-set-1.json";
    let mut over_long = testing::entry(name, &frame, flags::RESOURCE | flags::COMPRESSED);
    over_long.raw_len = u32::try_from(rules.len() - 1)?;
    let blob = testing::blob(&[over_long], false, 2);
    let view = View::new(&blob)?;
    assert!(matches!(
        Rules::load(view, "s3"),
        Err(RulesError::Decode(
            pyload::decompress::Error::OutputLengthMismatch
        ))
    ));
    assert!(matches!(
        Rules::load(view, "s3/../ec2"),
        Err(RulesError::InvalidServiceName)
    ));
    assert!(matches!(
        Rules::load(view, "ec2"),
        Err(RulesError::MissingEndpointRules)
    ));
    let plain = testing::entry(name, rules, flags::RESOURCE);
    let blob = testing::blob(&[plain], false, 2);
    let error = Rules::load(View::new(&blob)?, "s3")
        .err()
        .ok_or("no partitions loaded")?;
    assert!(
        matches!(error, RulesError::MissingEndpointRules),
        "{error:?}"
    );
    Ok(())
}
use crt::endpoints::ParamValue;
/// The S3 client's settings, as a plain client sets them.
const SETTINGS: [(&str, ParamValue<'_>); 6] = [
    ("UseFIPS", ParamValue::Boolean(false)),
    ("UseDualStack", ParamValue::Boolean(false)),
    ("ForcePathStyle", ParamValue::Boolean(false)),
    ("Accelerate", ParamValue::Boolean(false)),
    ("UseGlobalEndpoint", ParamValue::Boolean(false)),
    ("DisableMultiRegionAccessPoints", ParamValue::Boolean(false)),
];
pub(crate) fn payload() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let prefix = std::env::var_os("AWS_NATIVE_FALLBACK_PREFIX")
        .ok_or("AWS_NATIVE_FALLBACK_PREFIX must name the fallback payload prefix")?;
    Ok(std::fs::read(
        std::path::PathBuf::from(prefix).join("awscli-v2.blob"),
    )?)
}
#[test]
#[cfg_attr(miri, ignore = "calls the CRT, which Miri cannot run")]
fn embedded_s3_bucket_partition_endpoint_variants() -> Result<(), Box<dyn std::error::Error>> {
    let payload = payload()?;
    let view = View::new(&payload)?;
    for (region, bucket, url) in [
        (
            "us-west-2",
            Some("plain-bucket"),
            "https://plain-bucket.s3.us-west-2.amazonaws.com",
        ),
        (
            "us-east-1",
            Some("bucket.with.dots"),
            "https://s3.us-east-1.amazonaws.com/bucket.with.dots",
        ),
        ("cn-north-1", None, "https://s3.cn-north-1.amazonaws.com.cn"),
        ("us-east-1", None, "https://s3.us-east-1.amazonaws.com"),
    ] {
        let mut parameters = vec![("Region", ParamValue::String(region))];
        parameters.extend(bucket.map(|bucket| ("Bucket", ParamValue::String(bucket))));
        parameters.extend(SETTINGS);
        let resolved = resolve(view, "s3", &parameters)?;
        assert!(
            matches!(
                &resolved,
                crt::endpoints::Resolved::Endpoint { url: actual, .. } if actual == url.as_bytes()
            ),
            "{resolved:?}"
        );
    }
    Ok(())
}
#[test]
#[cfg_attr(miri, ignore = "calls the CRT, which Miri cannot run")]
fn resolutions_survive_runtime_libraries_released_around_them()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = payload()?;
    let rules = Rules::load(View::new(&payload)?, "s3")?;
    let mut parameters = vec![
        ("Region", ParamValue::String("us-west-2")),
        ("Bucket", ParamValue::String("plain-bucket")),
    ];
    parameters.extend(SETTINGS);
    std::thread::scope(|scope| {
        let resolving: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    (0..50).all(|_| {
                        matches!(
                            rules.resolve(&parameters),
                            Ok(crt::endpoints::Resolved::Endpoint { .. })
                        )
                    })
                })
            })
            .collect();
        for _ in 0..2 {
            scope.spawn(|| {
                for _ in 0..50 {
                    drop(crt::runtime::Library::acquire());
                }
            });
        }
        for resolver in resolving {
            assert!(matches!(resolver.join(), Ok(true)));
        }
    });
    Ok(())
}
