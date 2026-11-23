use super::*;

#[test]
fn patterns_match_as_the_prefix_trie_does() {
    for (pattern, event, matched) in [
        ("session-initialized", "session-initialized", true),
        (
            "building-argument-table",
            "building-argument-table.sts.get-caller-identity",
            true,
        ),
        (
            "building-argument-table.*",
            "building-argument-table.sts.get-caller-identity",
            true,
        ),
        (
            "building-argument-table.ec2.*",
            "building-argument-table.ec2.run-instances",
            true,
        ),
        (
            "building-argument-table.ec2.*",
            "building-argument-table.sts.get-caller-identity",
            false,
        ),
        ("doc-examples.*.*", "doc-examples.sts", false),
        ("after-call.s3", "after-call.s3.PutObject", true),
        ("after-call.s3", "after-call.s3-control.PutObject", false),
        (
            "calling-command.dynamodb.*",
            "calling-command.dynamodb",
            false,
        ),
    ] {
        assert_eq!(
            pattern_matches(pattern, event),
            matched,
            "{pattern} ~ {event}"
        );
    }
}

const DOCUMENT: &str = r#"{
  "plugin_registry": [
    ["session-initialized", [["awscli.paramfile", "register_init_uri_param_handler"]]],
    ["calling-command.zsvc.*", [["awscli.customizations.z", "fix"]]]
  ],
  "main_command_table_ops": [["rename", "old", "new"], ["add", "hand", "awscli.m", "C"]],
  "builtin_handlers": [["before-call.zsvc.Op", "botocore.handlers.h", "first"]],
  "client_handlers": {
    "generic": [
      ["request-created.{service_id}", "botocore.signers.RequestSigner.handler", "default"]
    ],
    "services": [["zsvc", [["before-parameter-build", "botocore.discovery.block", "default"]]]]
  }
}"#;

fn subject(service: &'static str, packed: bool) -> Subject<'static> {
    Subject {
        service,
        endpoint_prefix: service,
        service_id: service,
        command: service,
        operation: "Op",
        cli_operation: "op",
        arguments: &["thing"],
        packed,
    }
}

#[test]
fn the_entry_reads_as_the_generator_writes_it() -> Result<(), PatternError> {
    let patterns = Patterns::parse(DOCUMENT.as_bytes())?;
    assert_eq!(patterns.registry.len(), 2);
    assert_eq!(
        patterns.table_ops,
        [
            TableOp::Rename {
                from: "old".into(),
                to: "new".into()
            },
            TableOp::Add {
                name: "hand".into(),
                module: "awscli.m".into(),
                class: "C".into()
            },
        ]
    );
    assert_eq!(patterns.builtins[0].section, Section::First);
    assert_eq!(patterns.client.generic.len(), 1);
    assert_eq!(&*patterns.client.services[0].0, "zsvc");
    for broken in [&b"{}"[..], b"[]", br#"{"plugin_registry": 1}"#, b"not json"] {
        assert!(Patterns::parse(broken).is_err(), "{broken:?}");
    }
    Ok(())
}

#[test]
fn a_subject_meets_its_service_s_entries_and_the_session_s() -> Result<(), PatternError> {
    let patterns = Patterns::parse(DOCUMENT.as_bytes())?;
    let touched = patterns.touching(&subject("zsvc", true));
    let expected = [
        "awscli.customizations.z.fix",
        "awscli.paramfile.register_init_uri_param_handler",
        "botocore.discovery.block",
        "botocore.handlers.h",
        "botocore.signers.RequestSigner.handler",
    ];
    assert_eq!(
        touched.iter().map(String::as_str).collect::<Vec<_>>(),
        expected
    );
    let other = patterns.touching(&subject("other", true));
    assert_eq!(other.len(), 2);
    Ok(())
}

/// A client's generic rows name its own service where the pack wrote
/// `{service_id}`; its service's rows apply only to the model the pack
/// observed, so a user model of the same name meets the generic rows alone.
#[test]
fn client_rows_follow_the_service_and_the_observed_model() -> Result<(), PatternError> {
    let patterns = Patterns::parse(DOCUMENT.as_bytes())?;
    let rows = |subject: &Subject<'_>| {
        let handlers = patterns.client_handlers(subject, &subject.client_events());
        handlers.into_iter().collect::<Vec<_>>()
    };
    let generic = ["botocore.signers.RequestSigner.handler"];
    assert_eq!(rows(&subject("other", true)), generic);
    assert_eq!(rows(&subject("zsvc", false)), generic);
    assert_eq!(
        rows(&subject("zsvc", true)),
        [
            "botocore.discovery.block",
            "botocore.signers.RequestSigner.handler"
        ]
    );
    assert!(!pattern_matches(
        &patterns.client.generic[0].pattern,
        "request-created.zsvc.Op"
    ));
    Ok(())
}

#[test]
fn help_s_events_are_never_emitted() {
    let subject = subject("zsvc", true);
    let events = subject.cli_events();
    assert!(events.iter().all(|event| !event.starts_with("doc-")));
    assert!(events.contains(&"load-cli-arg.zsvc.op.thing".to_owned()));
    assert!(events.contains(&"building-command-table.zsvc_op".to_owned()));
    let client = subject.client_events();
    assert!(client.contains(&"after-call.zsvc.Op".to_owned()));
    assert!(client.iter().all(|event| !event.starts_with("getattr.")));
}
