use super::*;

const UUID: &str = "1bea5eb3-c152-4a22-9489-328f1be28e40";
const OTHER_UUID: &str = "4ad10757-8625-4a62-b493-5e3f800a6ab8";

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
        complete: true,
    }
}

fn one(request: &Request) -> Normal {
    let mut normal = requests(std::slice::from_ref(request), []);
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
    let authorization = |signed: &str| {
        header(
            "Authorization",
            &format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261125/us-west-2/zjson/aws4_request, \
                 SignedHeaders={signed}, \
                 Signature=5514791b4170c9a0122764bd3bc19731ee840a634ba46da4be8121d8c8d98328"
            ),
        )
    };
    assert_eq!(
        authorization("host;x-amz-date"),
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/<date>/us-west-2/zjson/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=<sig>"
    );
    assert_ne!(authorization("host;x-amz-date"), authorization("host"));
    assert_eq!(header("X-Amz-Date", "20261125T191630Z"), "<amz-date>");
    assert_eq!(header("x-amz-date", "x"), "<amz-date>");
}

#[test]
fn only_the_ttl_of_amz_sdk_request_goes() {
    assert_eq!(
        header("amz-sdk-request", "ttl=20261125T191630Z; attempt=2; max=3"),
        "ttl=<ttl>; attempt=2; max=3"
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
        super::requests(&requests, [])
            .into_iter()
            .flat_map(|normal| normal.headers.into_iter().map(|(_, value)| value))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(&["x", "x", "y"]),
        [
            "<invocation-id:1>",
            "<invocation-id:1>",
            "<invocation-id:2>"
        ]
    );
    assert_ne!(labels(&["x", "x"]), labels(&["x", "y"]));
}

#[test]
fn the_user_agent_loses_its_host_and_session_tokens_alone() {
    assert_eq!(
        header(
            "User-Agent",
            "aws-cli/2.40.0 md/awscrt#1.0.0.dev0 ua/2.1 os/linux#6.12.115 md/arch#x86_64 \
             lang/python#3.14.8 md/pyimpl#CPython m/Z,b,E,g cfg/retry-mode#standard \
             md/installer#other sid/b127ad166282 md/distrib#amzn.2023 md/fallback#env_override \
             md/prompt#off md/command#zjson.describe"
        ),
        "aws-cli/2.40.0 md/awscrt#1.0.0.dev0 ua/2.1 os/<os> md/arch#<arch> lang/python#3.14.8 \
         md/pyimpl#CPython m/E,Z,b,g cfg/retry-mode#standard md/installer#other sid/<sid> \
         md/distrib#<distrib> md/fallback#env_override md/prompt#off md/command#zjson.describe"
    );
    assert_eq!(
        header(
            "User-Agent",
            "aws-cli/2 os/other md/haiku#r1 md/arch#x86 lang/rust#1.96.0 \
             CRTS3NativeClient/0.1 platform/linux"
        ),
        "aws-cli/2 os/<os> md/arch#<arch> lang/rust#1.96.0 \
         CRTS3NativeClient/0.1 platform/<platform>"
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
    let given = requests(&[request("/", &[], &body)], ["--token", UUID]);
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
fn the_target_splits_into_host_path_and_normalized_query() {
    let target = format!(
        "/things/t?X-Amz-Date=20261125T1&X-Amz-Signature=ab12&\
         X-Amz-Credential=AKID%2F20261125%2Fus-west-2%2Fs3%2Faws4_request&token={UUID}&flag"
    );
    let normal = one(&request(
        &target,
        &[("Host", "zjson.us-west-2.amazonaws.com")],
        "",
    ));
    assert_eq!(normal.host, "zjson.us-west-2.amazonaws.com");
    assert_eq!(normal.path, "/things/t");
    let query: Vec<(&str, &str)> = normal
        .query
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        query,
        [
            ("X-Amz-Date", "<amz-date>"),
            ("X-Amz-Signature", "<sig>"),
            (
                "X-Amz-Credential",
                "AKID%2F<date>%2Fus-west-2%2Fs3%2Faws4_request"
            ),
            ("token", "<uuid:1>"),
            ("flag", ""),
        ]
    );
    let absolute = one(&request(
        "http://example.com:8080",
        &[("Host", "other")],
        "",
    ));
    assert_eq!(
        (absolute.host.as_str(), absolute.path.as_str()),
        ("example.com:8080", "/")
    );
}
