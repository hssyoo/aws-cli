//! The settings a model operation's client runs with, and the verdict of
//! each setting the generic path audits. The fake source's `sts` stands
//! for any client that is not S3's.
use super::*;
use cli_core::client_config::{Resolved, RetryMode, Source};

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}
/// The run of `sts get-caller-identity` under `pairs` and `config`.
fn identity_run(pairs: &[(&str, &str)], config: &str) -> Box<Run> {
    run_of(evaluate_with(IDENTITY, &env(pairs), &files(config)))
}
fn identity_verdict(pairs: &[(&str, &str)], config: &str) -> Verdict {
    evaluate_with(IDENTITY, &env(pairs), &files(config))
}
fn assert_python(verdict: Verdict, reason: PythonReason, case: &str) {
    assert!(
        matches!(verdict, Verdict::Python(actual) if actual == reason),
        "{case}: {verdict:?}, expected {reason:?}"
    );
}
fn assert_runs(verdict: Verdict, case: &str) {
    assert!(matches!(verdict, Verdict::Run(_)), "{case}: {verdict:?}");
}

#[test]
fn a_run_carries_its_resolved_settings_and_command_label() {
    let run = identity_run(&[], "");
    assert_eq!(run.settings.command, "sts.get-caller-identity");
    assert_eq!(run.settings.max_attempts, None);
    assert_eq!(run.settings.retry_mode, None);
    let configured = identity_run(&[("AWS_MAX_ATTEMPTS", "1")], "");
    assert_eq!(
        configured.settings.max_attempts,
        Some(Resolved::new(1, Source::Env("AWS_MAX_ATTEMPTS")))
    );
    let profile = identity_run(&[], "[default]\nmax_attempts = 1\nretry_mode = standard\n");
    assert_eq!(
        profile.settings.max_attempts,
        Some(Resolved::new(1, Source::Profile("max_attempts")))
    );
    assert_eq!(
        profile.settings.retry_mode,
        Some(Resolved::new(
            RetryMode::Standard,
            Source::Profile("retry_mode")
        ))
    );
}

/// One attempt is refused for the s3 commands, whose CRT client cannot
/// express it, and admitted for a model operation's client.
#[test]
fn one_attempt_is_native_for_the_model_client_only() {
    assert_runs(identity_verdict(&[("AWS_MAX_ATTEMPTS", "1")], ""), "sts");
    let listing = evaluate_with(
        &["s3", "ls", "s3://bucket"],
        &env(&[("AWS_MAX_ATTEMPTS", "1")]),
        &files(""),
    );
    assert_python(listing, PythonReason::RetrySetting, "s3 ls");
}

#[test]
fn retry_modes_other_than_standard_are_python_s() {
    for mode in ["adaptive", "legacy"] {
        let verdict = identity_verdict(&[("AWS_RETRY_MODE", mode)], "");
        assert_python(verdict, PythonReason::RetrySetting, mode);
    }
}

/// The four `s3` block keys botocore reads for an S3 client only refuse
/// the s3 commands as before and no other client.
#[test]
fn the_s3_block_s_unsupported_keys_refuse_only_the_s3_commands() {
    for (key, reason) in [
        ("force_path_style", PythonReason::EndpointSetting),
        ("accelerate", PythonReason::EndpointSetting),
        (
            "disable_multi_region_access_points",
            PythonReason::EndpointSetting,
        ),
        ("payload_signing_enabled", PythonReason::TransferSetting),
    ] {
        let config = format!("[default]\ns3 =\n  {key} = true\n");
        assert_runs(identity_verdict(&[], &config), key);
        for argv in [
            &["s3", "ls", "s3://bucket"][..],
            &["s3", "cp", "a", "s3://b/k"],
        ] {
            let verdict = evaluate_with(argv, &env(&[]), &files(&config));
            assert_python(verdict, reason, key);
        }
    }
}

#[test]
fn sts_regional_endpoints_is_ignored() {
    assert_runs(
        identity_verdict(&[("AWS_STS_REGIONAL_ENDPOINTS", "legacy")], ""),
        "env",
    );
    let config = "[default]\nsts_regional_endpoints = legacy\n";
    assert_runs(identity_verdict(&[], config), "profile");
}

#[test]
fn use_fips_endpoint_is_honoured() {
    let run = identity_run(&[("AWS_USE_FIPS_ENDPOINT", "true")], "");
    let expected = Resolved::new(true, Source::Env("AWS_USE_FIPS_ENDPOINT"));
    assert_eq!(run.settings.use_fips_endpoint, Some(expected));
}

#[test]
fn use_dualstack_endpoint_is_honoured() {
    let run = identity_run(&[], "[default]\nuse_dualstack_endpoint = true\n");
    let expected = Resolved::new(true, Source::Profile("use_dualstack_endpoint"));
    assert_eq!(run.settings.use_dualstack_endpoint, Some(expected));
}

/// botocore's default, `auto`, discovers only for required operations,
/// which admission refuses, and `false` never discovers; any other value,
/// from either source, discovers for optional operations too or is
/// botocore's error (`_normalize_endpoint_discovery_config`), so the
/// command is Python's.
#[test]
fn endpoint_discovery_enabled_is_python_s_unless_auto_or_false() {
    for (value, native) in [
        ("auto", true),
        ("false", true),
        ("true", false),
        ("AUTO", false),
        ("bogus", false),
        ("", false),
    ] {
        let verdict = identity_verdict(&[("AWS_ENDPOINT_DISCOVERY_ENABLED", value)], "");
        if native {
            assert_runs(verdict, value);
        } else {
            assert_python(verdict, PythonReason::ClientSetting, value);
        }
        let profile = format!("[default]\nendpoint_discovery_enabled = {value}\n");
        let verdict = identity_verdict(&[], &profile);
        if native {
            assert_runs(verdict, &profile);
        } else {
            assert_python(verdict, PythonReason::ClientSetting, &profile);
        }
    }
    // S3's clients have no discovery operation: the row does not reach them.
    let listing = ["s3", "ls", "s3://bucket"];
    let plain = evaluate_with(&listing, &env(&[]), &files(""));
    let set = evaluate_with(
        &listing,
        &env(&[("AWS_ENDPOINT_DISCOVERY_ENABLED", "true")]),
        &files(""),
    );
    assert_eq!(format!("{set:?}"), format!("{plain:?}"));
}

#[test]
fn request_checksum_calculation_is_honoured() {
    let run = identity_run(&[("AWS_REQUEST_CHECKSUM_CALCULATION", "WHEN_REQUIRED")], "");
    assert_eq!(
        run.settings.request_checksum.source,
        Source::Env("AWS_REQUEST_CHECKSUM_CALCULATION")
    );
    let verdict = identity_verdict(&[("AWS_REQUEST_CHECKSUM_CALCULATION", "always")], "");
    assert_python(verdict, PythonReason::TransferSetting, "invalid");
}

#[test]
fn response_checksum_validation_is_honoured() {
    let config = "[default]\nresponse_checksum_validation = when_required\n";
    let run = identity_run(&[], config);
    assert_eq!(
        run.settings.response_checksum.source,
        Source::Profile("response_checksum_validation")
    );
    let invalid = "[default]\nresponse_checksum_validation = never\n";
    assert_python(
        identity_verdict(&[], invalid),
        PythonReason::TransferSetting,
        "invalid",
    );
}

#[test]
fn disable_request_compression_is_ignored() {
    for value in ["true", "false", "bogus"] {
        let verdict = identity_verdict(&[("AWS_DISABLE_REQUEST_COMPRESSION", value)], "");
        assert_runs(verdict, value);
    }
}

/// `_validate_min_compression_size`: `int()` of the value, from 1 to 1 MiB.
#[test]
fn request_min_compression_size_bytes_is_validated() {
    let name = "AWS_REQUEST_MIN_COMPRESSION_SIZE_BYTES";
    for value in ["1", " 10240 ", "1048576"] {
        assert_runs(identity_verdict(&[(name, value)], ""), value);
    }
    for value in ["0", "1048577", "ten", ""] {
        assert_python(
            identity_verdict(&[(name, value)], ""),
            PythonReason::ClientSetting,
            value,
        );
    }
    let nested = "[default]\nrequest_min_compression_size_bytes =\n  nested = 1\n";
    assert_python(
        identity_verdict(&[], nested),
        PythonReason::ClientSetting,
        "block",
    );
}

#[test]
fn sigv4a_signing_region_set_is_ignored() {
    let verdict = identity_verdict(&[("AWS_SIGV4A_SIGNING_REGION_SET", "*")], "");
    assert_runs(verdict, "sigv4a");
}

#[test]
fn auth_scheme_preference_is_python_s() {
    let verdict = identity_verdict(&[("AWS_AUTH_SCHEME_PREFERENCE", "sigv4a,sigv4")], "");
    assert_python(verdict, PythonReason::ClientSetting, "env");
    let config = "[default]\nauth_scheme_preference = sigv4\n";
    assert_python(
        identity_verdict(&[], config),
        PythonReason::ClientSetting,
        "profile",
    );
}

#[test]
fn account_id_endpoint_mode_is_python_s() {
    let verdict = identity_verdict(&[("AWS_ACCOUNT_ID_ENDPOINT_MODE", "disabled")], "");
    assert_python(verdict, PythonReason::EndpointSetting, "account id");
}

/// Both values `add_timestamp_parser` knows run natively; any other fails
/// the session, which Python reports.
#[test]
fn cli_timestamp_format_is_native_as_iso8601_or_wire() {
    for value in ["iso8601", "wire"] {
        let config = format!("[default]\ncli_timestamp_format = {value}\n");
        assert_runs(identity_verdict(&[], &config), value);
    }
    assert_python(
        identity_verdict(&[], "[default]\ncli_timestamp_format = bogus\n"),
        PythonReason::OutputFormat,
        "bogus",
    );
}

#[test]
fn cli_binary_format_is_validated() {
    for value in ["base64", "raw-in-base64-out"] {
        let config = format!("[default]\ncli_binary_format = {value}\n");
        assert_runs(identity_verdict(&[], &config), value);
    }
    let config = "[default]\ncli_binary_format = hex\n";
    assert_python(
        identity_verdict(&[], config),
        PythonReason::ClientSetting,
        "hex",
    );
}

/// The pager depends on the terminal, which the router cannot see: the
/// invocation's preflight decides it (`format::pager::destination`).
#[test]
fn pager_settings_leave_the_verdict_to_dispatch() {
    for (pairs, config) in [
        (&[("AWS_PAGER", "less")][..], ""),
        (&[("PAGER", "more")], ""),
        (&[], "[default]\ncli_pager = cat\n"),
    ] {
        assert_runs(identity_verdict(pairs, config), "pager");
    }
}

/// The output settings are resolved once, here, and carried on the run;
/// the invocation reads them and judges nothing again.
#[test]
fn output_is_honoured() {
    let run = identity_run(
        &[("AWS_DEFAULT_OUTPUT", "text"), ("AWS_PAGER", "more")],
        "[default]\ncli_timestamp_format = wire\n",
    );
    assert_eq!(run.output.format, ::command::OutputFormat::Text);
    let aws_pager = run.output.pager.aws_pager.as_deref();
    assert_eq!(aws_pager, Some(std::ffi::OsStr::new("more")));
    // wire timestamps print as the response's text, whatever the zone.
    assert!(!run.output.timestamps.needs_local_zone());
    let verdict = identity_verdict(&[], "[default]\noutput = table\n");
    assert_python(verdict, PythonReason::OutputFormat, "table");
}

#[test]
fn api_versions_is_ignored() {
    let config = "[default]\napi_versions =\n  sts = 2011-06-15\n";
    assert_runs(identity_verdict(&[], config), "api_versions");
}

#[test]
fn cli_history_is_python_s() {
    let verdict = identity_verdict(&[], "[default]\ncli_history = enabled\n");
    assert_python(verdict, PythonReason::HistoryEnabled, "history");
}

/// Auto-prompt is decided from the literal argv after routing
/// (`cli_error::dispose_native`), for a run as for the s3 commands.
#[test]
fn cli_auto_prompt_leaves_the_verdict_to_dispatch() {
    assert_runs(
        identity_verdict(&[("AWS_CLI_AUTO_PROMPT", "on")], ""),
        "auto-prompt",
    );
}

/// `ClientArgsCreator.compute_client_args`: `ensure_boolean` of the
/// profile's value; anything but `true` turns botocore's validation off.
#[test]
fn parameter_validation_off_is_python_s() {
    assert_runs(
        identity_verdict(&[], "[default]\nparameter_validation = TRUE\n"),
        "on",
    );
    for value in ["false", "no"] {
        let config = format!("[default]\nparameter_validation = {value}\n");
        assert_python(
            identity_verdict(&[], &config),
            PythonReason::ClientSetting,
            value,
        );
    }
}

#[test]
fn tcp_keepalive_is_honoured() {
    let run = identity_run(&[], "[default]\ntcp_keepalive = true\n");
    assert!(run.settings.keep_alive.value);
    assert_eq!(
        run.settings.keep_alive.source,
        Source::Profile("tcp_keepalive")
    );
}

#[test]
fn the_data_path_variable_is_honoured_and_the_profile_key_ignored() {
    assert_runs(
        identity_verdict(&[("AWS_DATA_PATH", "/models")], ""),
        "variable",
    );
    assert_runs(
        identity_verdict(&[], "[default]\ndata_path = /models\n"),
        "profile",
    );
}

#[test]
fn the_read_timeout_is_native_for_a_formatted_command_only() {
    let run = run_of(evaluate_with(
        &["--cli-read-timeout", "5", "sts", "get-caller-identity"],
        &env(&[]),
        &files(""),
    ));
    assert_eq!(
        run.settings.read_timeout_ms,
        Resolved::new(5000, Source::Flag("--cli-read-timeout"))
    );
    let negative = ["--cli-read-timeout", "-5", "sts", "get-caller-identity"];
    let verdict = evaluate_with(&negative, &env(&[]), &files(""));
    assert_python(verdict, PythonReason::PythonOnlyGlobal, "negative");
    let listing = ["--cli-read-timeout", "5", "s3", "ls", "s3://bucket"];
    let verdict = evaluate_with(&listing, &env(&[]), &files(""));
    assert_python(verdict, PythonReason::PythonOnlyGlobal, "s3 ls");
}

/// Settings every client reads that change what the native client would
/// send: client-side monitoring, host-prefix injection, and an S3 Express
/// switch botocore validates for every client
/// (`_compute_s3_disable_express_session_auth`).
#[test]
fn other_client_wide_settings_are_python_s_where_they_change_the_request() {
    for (name, value, native) in [
        ("AWS_CSM_ENABLED", "true", false),
        ("AWS_CSM_ENABLED", "false", true),
        ("AWS_DISABLE_HOST_PREFIX_INJECTION", "true", false),
        ("AWS_DISABLE_HOST_PREFIX_INJECTION", "false", true),
        ("AWS_S3_DISABLE_EXPRESS_SESSION_AUTH", "maybe", false),
        ("AWS_S3_DISABLE_EXPRESS_SESSION_AUTH", "TRUE", true),
    ] {
        let verdict = identity_verdict(&[(name, value)], "");
        let case = format!("{name}={value}");
        if native {
            assert_runs(verdict, &case);
        } else {
            assert_python(verdict, PythonReason::ClientSetting, &case);
        }
    }
}
