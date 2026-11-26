use super::*;

const UUID: &str = "1bea5eb3-c152-4a22-9489-328f1be28e40";
const OTHER_UUID: &str = "4ad10757-8625-4a62-b493-5e3f800a6ab8";
const DATE: &str = "20261125T191630Z";
const SIGNATURE: &str = "5514791b4170c9a0122764bd3bc19731ee840a634ba46da4be8121d8c8d98328";

fn request(target: &str, headers: &[(&str, &str)], body: &str) -> Request {
    Request {
        method: "POST".to_owned(),
        target: target.to_owned(),
        version: "1.1".to_owned(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body: body.as_bytes().to_vec(),
        framing: None,
        complete: true,
    }
}

fn one(request: &Request) -> Normal {
    let mut normal = requests(std::slice::from_ref(request), [], None);
    normal.pop().unwrap_or_default()
}

fn header(name: &str, value: &str) -> String {
    let normal = one(&request("/", &[(name, value)], ""));
    normal
        .headers
        .first()
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

#[test]
fn the_signature_and_scope_date_go_and_the_signed_headers_stay() {
    let authorization = |signed: &str, signature: &str| {
        header(
            "Authorization",
            &format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261125/us-west-2/zjson/aws4_request, \
                 SignedHeaders={signed}, Signature={signature}"
            ),
        )
    };
    assert_eq!(
        authorization("host;x-amz-date", SIGNATURE),
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/<date>/us-west-2/zjson/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=<sig:unverified>"
    );
    assert_ne!(
        authorization("host;x-amz-date", SIGNATURE),
        authorization("host", SIGNATURE)
    );
    for malformed in ["", "abc", &SIGNATURE.to_ascii_uppercase(), &SIGNATURE[1..]] {
        assert!(authorization("host", malformed).ends_with(&format!("Signature={malformed}")));
    }
    assert_eq!(header("X-Amz-Date", DATE), "<amz-date>");
    for malformed in ["garbage", "", "20261125T191630", "2026112xT191630Z"] {
        assert_eq!(header("X-Amz-Date", malformed), malformed);
    }
}

#[test]
fn the_signature_becomes_its_verdict_with_the_runs_secret() {
    let vanilla = request(
        "/",
        &[
            ("Host", "example.amazonaws.com"),
            ("X-Amz-Date", "20150830T123600Z"),
            (
                "Authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=host;x-amz-date, \
                 Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
            ),
        ],
        "",
    );
    let vanilla = Request {
        method: "GET".to_owned(),
        ..vanilla
    };
    let verdict = |secret| {
        let normal = requests(std::slice::from_ref(&vanilla), [], Some(secret));
        let authorization = normal
            .first()
            .and_then(|normal| normal.headers.get(2).cloned());
        authorization.map(|(_, value)| value).unwrap_or_default()
    };
    assert!(verdict("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY").ends_with("Signature=<sig:ok>"));
    assert!(verdict("secret").ends_with("Signature=<sig:bad>"));
}

#[test]
fn only_a_ttl_date_of_amz_sdk_request_goes() {
    assert_eq!(
        header("amz-sdk-request", &format!("ttl={DATE}; attempt=2; max=3")),
        "ttl=<ttl>; attempt=2; max=3"
    );
    assert_eq!(
        header("amz-sdk-request", "ttl=; attempt=1"),
        "ttl=; attempt=1"
    );
    assert_ne!(
        header("amz-sdk-request", "attempt=1; max=3"),
        header("amz-sdk-request", "attempt=1; max=7")
    );
}

#[test]
fn invocation_ids_are_numbered_by_first_appearance() {
    let sent = |id: &str| request("/", &[("amz-sdk-invocation-id", id)], "");
    let labels = |ids: &[&str]| {
        let requests: Vec<_> = ids.iter().map(|id| sent(id)).collect();
        super::requests(&requests, [], None)
            .into_iter()
            .flat_map(|normal| normal.headers.into_iter().map(|(_, value)| value))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(&[UUID, UUID, OTHER_UUID]),
        [
            "<invocation-id:1>",
            "<invocation-id:1>",
            "<invocation-id:2>"
        ]
    );
    assert_ne!(labels(&[UUID, UUID]), labels(&[UUID, OTHER_UUID]));
    assert_eq!(labels(&["", "x"]), ["", "x"]);
}

#[test]
fn the_user_agent_loses_only_releases_versions_and_its_session() {
    assert_eq!(
        header(
            "User-Agent",
            "aws-cli/2.40.0 md/awscrt#1.0.0.dev0 ua/2.1 os/linux#6.12.115 md/arch#x86_64 \
             lang/python#3.14.8 md/pyimpl#CPython m/Z,b,E,g cfg/retry-mode#standard \
             md/installer#other sid/b127ad166282 md/distrib#amzn.2023 md/fallback#env_override \
             md/prompt#off md/command#zjson.describe"
        ),
        "aws-cli/2.40.0 md/awscrt#1.0.0.dev0 ua/2.1 os/linux#<release> md/arch#x86_64 \
         lang/python#3.14.8 md/pyimpl#CPython m/E,Z,b,g cfg/retry-mode#standard \
         md/installer#other sid/<sid> md/distrib#amzn.<version> md/fallback#env_override \
         md/prompt#off md/command#zjson.describe"
    );
    assert_eq!(
        header(
            "User-Agent",
            "aws-cli/2 os/other md/haiku#r1 md/arch#x86 lang/rust#1.96.0 \
             CRTS3NativeClient/0.1 platform/linux sid/ sid/xyz"
        ),
        "aws-cli/2 os/other md/haiku#<release> md/arch#x86 lang/rust#1.96.0 \
         CRTS3NativeClient/0.1 platform/linux sid/ sid/xyz"
    );
    assert_ne!(
        header("User-Agent", "os/linux#6"),
        header("User-Agent", "os/macos#6")
    );
}

#[test]
fn generated_uuids_are_numbered_and_given_ones_stay() {
    let body =
        format!("{{\"Token\": \"{UUID}\", \"Other\": \"{OTHER_UUID}\", \"Again\": \"{UUID}\"}}");
    let normal = one(&request("/", &[], &body));
    assert_eq!(
        String::from_utf8_lossy(&normal.body),
        "{\"Token\": \"<uuid:1>\", \"Other\": \"<uuid:2>\", \"Again\": \"<uuid:1>\"}"
    );
    let given = requests(&[request("/", &[], &body)], ["--token", UUID], None);
    let kept = given
        .first()
        .map(|normal| String::from_utf8_lossy(&normal.body).into_owned());
    let expected =
        format!("{{\"Token\": \"{UUID}\", \"Other\": \"<uuid:1>\", \"Again\": \"{UUID}\"}}");
    assert_eq!(kept, Some(expected));
}

#[test]
fn other_hex_runs_are_not_uuids() {
    let version_1 = "1bea5eb3-c152-1a22-9489-328f1be28e40";
    let longer = format!("a{UUID}");
    for body in [version_1, longer.as_str(), &UUID[..35]] {
        assert_eq!(
            one(&request("/", &[], body)).body,
            body.as_bytes(),
            "{body}"
        );
    }
    assert_eq!(uuids(UUID.to_ascii_uppercase().as_bytes()), [0]);
}

#[test]
fn the_target_keeps_its_fields_and_normalizes_their_values() {
    let target = format!(
        "/things/t?X-Amz-Date={DATE}&X-Amz-Signature={SIGNATURE}&\
         X-Amz-Credential=AKID%2F20261125%2Fus-west-2%2Fs3%2Faws4_request&token={UUID}&flag&&empty="
    );
    let normal = one(&request(
        &target,
        &[("Host", "zjson.us-west-2.amazonaws.com")],
        "",
    ));
    assert_eq!(normal.host, "zjson.us-west-2.amazonaws.com");
    assert_eq!(normal.path, "/things/t");
    let query = "X-Amz-Date=<amz-date>&X-Amz-Signature=<sig>&\
                 X-Amz-Credential=AKID%2F<date>%2Fus-west-2%2Fs3%2Faws4_request&\
                 token=<uuid:1>&flag&&empty=";
    assert_eq!(normal.query.as_deref(), Some(query));
    assert_eq!(normal.target, format!("/things/t?{query}"));
    let kept = one(&request("/?X-Amz-Signature=short&X-Amz-Date=soon", &[], ""));
    assert_eq!(
        kept.query.as_deref(),
        Some("X-Amz-Signature=short&X-Amz-Date=soon")
    );
    let bare = one(&request("/b?", &[], ""));
    assert_eq!(
        (bare.path.as_str(), bare.query.as_deref()),
        ("/b", Some(""))
    );
    let absolute = one(&request(
        "http://example.com:8080",
        &[("Host", "other")],
        "",
    ));
    let parts = (
        absolute.host.as_str(),
        absolute.path.as_str(),
        absolute.target.as_str(),
    );
    assert_eq!(parts, ("example.com:8080", "", "http://example.com:8080"));
}

#[test]
fn fields_keep_a_missing_equals_and_empty_fields() {
    let pair = |name: &str, value: Option<&str>| (name.to_owned(), value.map(str::to_owned));
    let read = fields("&k&e=&a=1&&b=2=3&");
    assert_eq!(
        read.pairs,
        [
            pair("k", None),
            pair("e", Some("")),
            pair("a", Some("1")),
            pair("b", Some("2=3"))
        ]
    );
    let after = |name: &str| Some((name.to_owned(), 0));
    assert_eq!(read.empty, [None, after("a"), after("b")]);
    assert_eq!(fields(""), Fields::default());
    assert_eq!(fields("&").empty, [None, None]);
    let moved = (fields("a=1&&b=2&c=1").empty, fields("a=1&b=2&&c=2").empty);
    assert_ne!(moved.0, moved.1);
    assert_eq!(fields("a&a&&b").empty, [Some(("a".to_owned(), 1))]);
}

// B3.3 (G-042a): each fails on 17a368a34, where a header's UUID stayed raw.

/// The headers of `request` after normalizing it alone, with `inputs`.
fn headers_of(request: &Request, inputs: &[&str]) -> Vec<(String, String)> {
    let mut normal = requests(std::slice::from_ref(request), inputs.iter().copied(), None);
    normal.pop().unwrap_or_default().headers
}

#[test]
fn a_generated_uuid_in_a_header_is_numbered() {
    let sent = |uuid: &str| {
        headers_of(
            &request("/", &[("x-amz-client-token", uuid), ("X-Other", "t-1")], ""),
            &[],
        )
    };
    assert_eq!(sent(UUID), sent(OTHER_UUID));
    assert_eq!(
        sent(UUID),
        [
            ("x-amz-client-token".to_owned(), "<uuid:1>".to_owned()),
            ("X-Other".to_owned(), "t-1".to_owned())
        ]
    );
    // Inside a longer value too, as in a body.
    let within = headers_of(&request("/", &[("X-Ids", &format!("a,{UUID}"))], ""), &[]);
    assert_eq!(within, [("X-Ids".to_owned(), "a,<uuid:1>".to_owned())]);
}

#[test]
fn a_header_and_a_body_share_one_numbering() {
    // Evasion: a constant `<uuid>` would hide a side that sends two tokens
    // where the other sends one token twice.
    let sent = |header: &str, body: &str| {
        let normal = requests(
            &[request("/", &[("X-Token", header)], &format!("\"{body}\""))],
            [],
            None,
        );
        normal
            .first()
            .map(|normal| (normal.headers.clone(), normal.body.clone()))
    };
    assert_ne!(sent(UUID, UUID), sent(UUID, OTHER_UUID));
    let numbered = sent(UUID, UUID).map(|(_, body)| body);
    assert_eq!(numbered, Some(b"\"<uuid:1>\"".to_vec()));
    // Across requests, a retry that keeps its token still matches only one
    // that does.
    let retried = |second: &str| {
        let normal = requests(
            &[
                request("/", &[("X-Token", UUID)], ""),
                request("/", &[("X-Token", second)], ""),
            ],
            [],
            None,
        );
        normal.into_iter().map(|n| n.headers).collect::<Vec<_>>()
    };
    assert_ne!(retried(UUID), retried(OTHER_UUID));
}

#[test]
fn a_header_uuid_the_row_gives_stays() {
    // Evasion: normalizing the user's own token would hide a CLI that drops
    // it for a generated one.
    let given = headers_of(
        &request("/", &[("x-amz-client-token", UUID)], ""),
        &["--client-token", UUID],
    );
    assert_eq!(given, [("x-amz-client-token".to_owned(), UUID.to_owned())]);
    let generated = headers_of(
        &request("/", &[("x-amz-client-token", OTHER_UUID)], ""),
        &["--client-token", UUID],
    );
    assert_ne!(given, generated);
}

#[test]
fn a_header_value_that_is_not_a_v4_uuid_stays() {
    // Evasion: a version 1 UUID, a longer hex run, or a truncated one is not
    // a generated token, and must show when it changes.
    let version_1 = "1bea5eb3-c152-1a22-9489-328f1be28e40";
    let longer = format!("{UUID}0");
    for value in [version_1, longer.as_str(), &UUID[..35]] {
        assert_eq!(header("X-Token", value), value, "{value}");
    }
    // The invocation id keeps its own numbering, apart from the tokens.
    let normal = headers_of(
        &request(
            "/",
            &[("amz-sdk-invocation-id", UUID), ("X-Token", UUID)],
            "",
        ),
        &[],
    );
    let values: Vec<&str> = normal.iter().map(|(_, value)| value.as_str()).collect();
    assert_eq!(values, ["<invocation-id:1>", "<uuid:1>"]);
}

#[test]
fn a_header_uuid_does_not_hide_a_changed_signature() {
    // The signature is verified on the bytes as sent, before any header is
    // normalized: a token changed after signing is still bad.
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261125/us-west-2/zjson/aws4_request, \
         SignedHeaders=host;x-token, Signature={SIGNATURE}"
    );
    let sent = request(
        "/",
        &[
            ("Host", "h"),
            ("X-Token", UUID),
            ("Authorization", &authorization),
        ],
        "",
    );
    let normal = requests(std::slice::from_ref(&sent), [], Some("secret"));
    let headers = normal
        .first()
        .map(|normal| normal.headers.clone())
        .unwrap_or_default();
    let values: Vec<&str> = headers.iter().map(|(_, value)| value.as_str()).collect();
    assert_eq!(values.get(1), Some(&"<uuid:1>"));
    assert!(
        values
            .get(2)
            .is_some_and(|value| value.ends_with("Signature=<sig:bad>")),
        "{values:?}"
    );
}
