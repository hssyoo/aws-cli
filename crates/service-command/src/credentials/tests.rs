use super::*;

fn files(config: &str) -> ParsedFiles {
    ParsedFiles::parse(awscli_config::Inputs {
        config: Some(awscli_config::Contents {
            source: "config",
            bytes: config.as_bytes(),
        }),
        ..Default::default()
    })
}

fn ambient() -> Selected {
    Selected {
        name: "default".into(),
        explicit: false,
    }
}

/// The chain reaches the metadata services only when nothing before them
/// answers; a configured process answers first, without being run.
#[test]
fn only_an_unanswered_chain_reaches_the_metadata_services() {
    let reaches = |pairs: &[(&str, &str)], config: &str| {
        reaches_metadata(
            &SnapshotEnv::from_pairs(pairs.iter().copied()),
            &files(config),
            &ambient(),
        )
    };
    let keys = [("AWS_ACCESS_KEY_ID", "a"), ("AWS_SECRET_ACCESS_KEY", "b")];
    assert!(!reaches(&keys, ""));
    let static_keys = "[default]\naws_access_key_id = a\naws_secret_access_key = b\n";
    assert!(!reaches(&[], static_keys));
    // Never run: the path names nothing.
    assert!(!reaches(
        &[],
        "[default]\ncredential_process = /nonexistent/never-run\n"
    ));
    assert!(reaches(&[], ""));
    assert!(!reaches(&[("AWS_EC2_METADATA_DISABLED", "true")], ""));
    let container = [
        ("AWS_EC2_METADATA_DISABLED", "true"),
        ("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "/creds"),
    ];
    assert!(reaches(&container, ""));
}

/// Building the source runs nothing; the first request for credentials
/// walks the chain once and names the provider that answered.
#[test]
#[cfg(unix)]
fn the_chain_is_walked_once_on_the_first_request() -> std::io::Result<()> {
    let directory = std::env::temp_dir().join(format!("lazy-chain-{}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let runs = directory.join("runs");
    let script = directory.join("process.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho run >> {}\nprintf '{{\"Version\": 1, \"AccessKeyId\": \"k\", \
             \"SecretAccessKey\": \"s\"}}'\n",
            runs.display()
        ),
    )?;
    let mut permissions = std::fs::metadata(&script)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&script, permissions)?;
    let config = format!("[default]\ncredential_process = {}\n", script.display());
    let chain = LazyChain::new(&SnapshotEnv::default(), &files(&config), ambient());
    assert!(!runs.exists());
    assert_eq!(chain.method(), None);
    let first = chain.credentials();
    assert!(
        matches!(&first, Ok(Some(found)) if found.access_key_id == "k"),
        "{first:?}"
    );
    let _ = chain.credentials();
    assert_eq!(std::fs::read_to_string(&runs)?.lines().count(), 1);
    assert_eq!(chain.method(), Some("custom-process"));
    std::fs::remove_dir_all(&directory)
}
