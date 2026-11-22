//! The settings of a model operation's client: what botocore reads and
//! refuses for any client, and what the S3 client's record adds or refuses
//! on its own.
use super::*;
use std::collections::BTreeMap;

const SERVICE: &str = "zsvc";

fn settings_with(
    config: &str,
    entries: &[(&str, &str)],
    globals: &Globals,
) -> Result<ClientSettings, PythonReason> {
    let env = entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect::<BTreeMap<_, _>>();
    let files = ParsedFiles::parse(awscli_config::Inputs {
        config: Some(awscli_config::Contents {
            source: "config",
            bytes: config.as_bytes(),
        }),
        ..Default::default()
    });
    resolve_settings(&SettingsInputs {
        env: &env,
        files: &files,
        globals,
        directory: Path::new(env!("CARGO_MANIFEST_DIR")),
        embedded_trust: true,
        service: SERVICE,
        command: "zsvc.get-thing",
    })
}
fn settings(config: &str, entries: &[(&str, &str)]) -> Result<ClientSettings, PythonReason> {
    settings_with(config, entries, &Globals::default())
}

#[test]
fn nothing_configured_leaves_the_customizable_settings_unset() -> Result<(), PythonReason> {
    let resolved = settings("", &[])?;
    assert_eq!(resolved.command, "zsvc.get-thing");
    assert_eq!(resolved.retry_mode, None);
    assert_eq!(resolved.max_attempts, None);
    assert_eq!(
        resolved.retry_mode(),
        Resolved::new(RetryMode::Standard, Source::Default)
    );
    assert_eq!(resolved.use_fips_endpoint, None);
    assert_eq!(resolved.use_dualstack_endpoint, None);
    assert_eq!(resolved.connect_timeout_ms.value, 60_000);
    assert_eq!(resolved.read_timeout_ms.value, 60_000);
    assert_eq!(resolved.trust.value, Trust::Embedded);
    assert!(resolved.signing.value);
    assert!(!resolved.endpoint_override);
    Ok(())
}

/// The CRT S3 client cannot express one attempt; botocore and the model
/// client's own retry loop can, and botocore puts no upper bound on it.
#[test]
fn any_attempt_count_botocore_accepts_is_kept_with_its_source() -> Result<(), PythonReason> {
    for (count, attempts) in [("1", 1), ("65", 65), (" 7 ", 7)] {
        let resolved = settings("", &[("AWS_MAX_ATTEMPTS", count)])?;
        let expected = Resolved::new(attempts, Source::Env("AWS_MAX_ATTEMPTS"));
        assert_eq!(resolved.max_attempts, Some(expected), "{count:?}");
    }
    let profile = settings("[default]\nmax_attempts = 1\n", &[])?;
    let expected = Resolved::new(1, Source::Profile("max_attempts"));
    assert_eq!(profile.max_attempts, Some(expected));
    for count in ["", "0", "-1", "bad"] {
        assert_eq!(
            settings("", &[("AWS_MAX_ATTEMPTS", count)]),
            Err(PythonReason::RetrySetting),
            "{count:?}"
        );
    }
    Ok(())
}

/// `Config._validate_retry_configuration` allows `standard` and `adaptive`;
/// `legacy` is InvalidRetryModeError. The model client runs standard only.
#[test]
fn only_the_standard_retry_mode_is_native() -> Result<(), PythonReason> {
    let standard = settings("", &[("AWS_RETRY_MODE", "standard")])?;
    let expected = Resolved::new(RetryMode::Standard, Source::Env("AWS_RETRY_MODE"));
    assert_eq!(standard.retry_mode, Some(expected));
    for mode in ["adaptive", "legacy", "Standard", ""] {
        assert_eq!(
            settings("", &[("AWS_RETRY_MODE", mode)]),
            Err(PythonReason::RetrySetting),
            "{mode:?}"
        );
    }
    assert_eq!(
        settings("[default]\nretry_mode = adaptive\n", &[]),
        Err(PythonReason::RetrySetting)
    );
    Ok(())
}

#[test]
fn endpoint_variants_keep_unset_apart_from_false() -> Result<(), PythonReason> {
    let resolved = settings(
        "[default]\nuse_fips_endpoint = TRUE\nuse_dualstack_endpoint = false\n",
        &[],
    )?;
    assert_eq!(
        resolved.use_fips_endpoint,
        Some(Resolved::new(true, Source::Profile("use_fips_endpoint")))
    );
    assert_eq!(
        resolved.use_dualstack_endpoint,
        Some(Resolved::new(
            false,
            Source::Profile("use_dualstack_endpoint")
        ))
    );
    let variable = settings(
        "[default]\nuse_fips_endpoint = true\n",
        &[("AWS_USE_FIPS_ENDPOINT", "")],
    )?;
    assert_eq!(
        variable.use_fips_endpoint,
        Some(Resolved::new(false, Source::Env("AWS_USE_FIPS_ENDPOINT")))
    );
    Ok(())
}

/// globalargs.py `_resolve_timeout` and urllib3's timeout checks, as for the
/// connect timeout.
#[test]
fn the_read_timeout_follows_the_connect_timeout_s_rules() -> Result<(), PythonReason> {
    for (seconds, millis) in [(0, u32::MAX), (5, 5000)] {
        let globals = Globals {
            read_timeout: Some(seconds),
            ..Globals::default()
        };
        let resolved = settings_with("", &[], &globals)?;
        let expected = Resolved::new(millis, Source::Flag("--cli-read-timeout"));
        assert_eq!(resolved.read_timeout_ms, expected, "{seconds}");
    }
    let negative = Globals {
        read_timeout: Some(-1),
        ..Globals::default()
    };
    assert_eq!(
        settings_with("", &[], &negative),
        Err(PythonReason::PythonOnlyGlobal)
    );
    Ok(())
}

/// Every client's `Config` validates the `s3` block's addressing style
/// (`Config._validate_s3_configuration`, through `compute_client_args`);
/// the block's other keys only feed S3's endpoint rules.
#[test]
fn every_client_validates_the_s3_block_s_addressing_style() -> Result<(), PythonReason> {
    assert_eq!(
        settings("[default]\ns3 =\n  addressing_style = sideways\n", &[]),
        Err(PythonReason::EndpointSetting)
    );
    let block = "[default]\ns3 =\n  addressing_style = path\n  use_accelerate_endpoint = true\n  \
        force_path_style = true\n  payload_signing_enabled = true\n";
    settings(block, &[])?;
    // A key that is no block is no `s3` configuration (`SectionConfigProvider`).
    settings("[default]\ns3 = path\n", &[])?;
    Ok(())
}

#[test]
fn client_wide_refusals_apply_to_a_model_operation_s_client() {
    let long = "a".repeat(user_agent::SUPPLIED_BYTES_MAX + 1);
    for (config, entries, reason) in [
        (
            "",
            vec![("AWS_REQUEST_CHECKSUM_CALCULATION", "sometimes")],
            PythonReason::TransferSetting,
        ),
        (
            "[default]\nresponse_checksum_validation = never\n",
            vec![],
            PythonReason::TransferSetting,
        ),
        (
            "",
            vec![("AWS_CA_BUNDLE", " ")],
            PythonReason::NetworkSetting,
        ),
        (
            "",
            vec![("AWS_CA_BUNDLE", "missing.pem")],
            PythonReason::NetworkSetting,
        ),
        // A regular file that holds no certificate.
        (
            "",
            vec![("AWS_CA_BUNDLE", "Cargo.toml")],
            PythonReason::NetworkSetting,
        ),
        (
            "[default]\ntcp_keepalive =\n  nested = yes\n",
            vec![],
            PythonReason::NetworkSetting,
        ),
        (
            "",
            vec![("AWS_SDK_UA_APP_ID", long.as_str())],
            PythonReason::UserAgentSetting,
        ),
    ] {
        assert_eq!(
            settings(config, &entries),
            Err(reason),
            "{config:?} {entries:?}"
        );
    }
}

/// The configured-endpoint lookup names the client's own service.
#[test]
fn an_endpoint_for_the_client_s_service_is_an_override() -> Result<(), PythonReason> {
    let own = settings("", &[("AWS_ENDPOINT_URL_ZSVC", "https://example.com")])?;
    assert!(own.endpoint_override);
    let other = settings("", &[("AWS_ENDPOINT_URL_S3", "https://example.com")])?;
    assert!(!other.endpoint_override);
    Ok(())
}
