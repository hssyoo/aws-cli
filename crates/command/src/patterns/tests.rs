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
  },
  "command_table_edits": [
    [["awscli.r", "remove"], [
      ["building-command-table.zsvc", "awscli.r._remove", [["del", "gone"], ["del", "absent"]]],
      ["building-command-table.zunlisted", "awscli.r._remove", [["del", "op"]]]
    ]],
    [["awscli.w", "add"], [
      ["building-command-table.zother", "awscli.w._add", [["set", "wizard"]]],
      ["building-command-table.zsvc", "awscli.w._add", [["set", "wizard"]]]
    ]]
  ]
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

/// Every awscli entry and botocore handler that touches `subject`, by name.
fn touching(patterns: &Patterns, subject: &Subject<'_>) -> BTreeSet<String> {
    patterns
        .matches(subject)
        .into_iter()
        .map(|found| found.name)
        .collect()
}

/// A root's table edits are its listed editors' handlers on its own event,
/// in the registry's order; an editor's handler on a root it is not listed
/// under is never initialized for it, so it edits nothing there.
#[test]
fn table_edits_follow_the_editors_listed_under_the_root_s_event() -> Result<(), PatternError> {
    let mut document: Json =
        serde_json::from_str(DOCUMENT).map_err(|error| PatternError::Json(error.to_string()))?;
    document["plugin_registry"] = serde_json::json!([
        [
            "building-command-table.zsvc",
            [["awscli.r", "remove"], ["awscli.w", "add"]]
        ],
        ["building-command-table.zother", [["awscli.w", "add"]]]
    ]);
    let patterns = Patterns::parse(document.to_string().as_bytes())?;
    let set = |name: &str| TableEdit::Set(name.into());
    let delete = |name: &str| TableEdit::Delete(name.into());
    let edits = |command| -> Vec<TableEdit> {
        patterns.table_edits(command).into_iter().cloned().collect()
    };
    assert_eq!(
        edits("zsvc"),
        [delete("gone"), delete("absent"), set("wizard")]
    );
    assert_eq!(edits("zother"), [set("wizard")]);
    assert_eq!(edits("zunlisted"), []);
    assert_eq!(edits("nosuch"), []);
    for broken in [
        serde_json::json!([["a", "f"], []]),
        serde_json::json!([[["a", "f"], [["e", "h", [["move", "x"]]]]]]),
        Json::Null,
    ] {
        document["command_table_edits"] = broken.clone();
        let bytes = document.to_string();
        assert!(Patterns::parse(bytes.as_bytes()).is_err(), "{broken}");
    }
    Ok(())
}

#[test]
fn a_subject_meets_its_service_s_entries_and_the_session_s() -> Result<(), PatternError> {
    let patterns = Patterns::parse(DOCUMENT.as_bytes())?;
    let touched = touching(&patterns, &subject("zsvc", true));
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
    let other = touching(&patterns, &subject("other", true));
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

/// Each match names the pattern it matched by and where it came from, so
/// coverage can be judged per pattern: a registry entry matched by two
/// patterns is two matches.
#[test]
fn matches_carry_their_pattern_and_origin() -> Result<(), PatternError> {
    let patterns = Patterns::parse(DOCUMENT.as_bytes())?;
    let matches = patterns.matches(&subject("zsvc", true));
    let found: Vec<(MatchKind, &str, &str)> = matches
        .iter()
        .map(|found| (found.kind, &*found.pattern, &*found.name))
        .collect();
    assert_eq!(
        found,
        [
            (
                MatchKind::Registry,
                "session-initialized",
                "awscli.paramfile.register_init_uri_param_handler"
            ),
            (
                MatchKind::Registry,
                "calling-command.zsvc.*",
                "awscli.customizations.z.fix"
            ),
            (
                MatchKind::Builtin,
                "before-call.zsvc.Op",
                "botocore.handlers.h"
            ),
            (
                MatchKind::Client,
                "request-created.zsvc",
                "botocore.signers.RequestSigner.handler"
            ),
            (
                MatchKind::Client,
                "before-parameter-build",
                "botocore.discovery.block"
            ),
        ]
    );
    Ok(())
}
