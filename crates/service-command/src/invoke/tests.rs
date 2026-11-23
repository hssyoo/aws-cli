use super::*;

/// The payload's bundle is trusted only for the embedded setting, and a
/// configured file always wins.
#[test]
fn trust_follows_the_setting_and_the_payload() {
    let mut settings = ClientSettings::new("zquery.get-thing");
    settings.trust.value = Trust::Embedded;
    assert_eq!(
        trust(&settings, Some(b"pem")),
        client::transport::crt::Trust::Pem(b"pem".to_vec())
    );
    assert_eq!(
        trust(&settings, None),
        client::transport::crt::Trust::Platform
    );
    settings.trust.value = Trust::Platform;
    assert_eq!(
        trust(&settings, Some(b"pem")),
        client::transport::crt::Trust::Platform
    );
}

/// The preflight runs no credential provider, so nothing it or the client's
/// `prepare` refuses can have run one before Python runs the chain again:
/// a `credential_process` runs first when the client signs.
#[test]
#[cfg(unix)]
fn the_preflight_runs_no_credential_provider() -> crate::testing::Result<()> {
    let directory = std::env::temp_dir().join(format!("preflight-{}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let runs = directory.join("runs");
    let script = directory.join("process.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho run >> {}\nexit 1\n", runs.display()),
    )?;
    let mut permissions = std::fs::metadata(&script)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&script, permissions)?;
    let config = format!("[default]\ncredential_process = {}\n", script.display());
    let files = crate::testing::files(&config, "");
    let verdict = crate::testing::route(&["zjson", "ping"], &[], &files)?;
    let cli_router::Verdict::Run(run) = verdict else {
        return Err(format!("not native: {verdict:?}").into());
    };
    let env = cli_core::SnapshotEnv::from_pairs([("AWS_DEFAULT_REGION", "us-east-1")]);
    let variables = std::collections::HashMap::new();
    let preflight = Preflight {
        env: &env,
        variables: &variables,
        shared_files: &files,
        globals: &run.globals,
        directory: &directory,
        settings: &run.settings,
        output: &run.output,
        stdout_is_tty: false,
    };
    assert_eq!(run.invocation.preflight(&preflight), Ok(()));
    assert!(!runs.exists(), "the preflight ran the process");
    std::fs::remove_dir_all(&directory)?;
    Ok(())
}
