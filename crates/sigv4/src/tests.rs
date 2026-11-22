use super::*;
use timestamp::Timestamp;

const SUITE_KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
/// 20150830T123600Z, the AWS SigV4 test suite's date.
const SUITE_DATE: i64 = 1_440_938_160;
/// 20160818T143303Z, the date upstream's `test_presign_command.py` freezes
/// the clock at.
const PRESIGN_DATE: i64 = 1_471_530_783;

/// `seconds` as a timestamp.
#[expect(
    clippy::expect_used,
    reason = "every instant the tests name is within the timestamp's range"
)]
fn at(seconds: i64) -> Timestamp {
    Timestamp::from_unix_seconds(seconds).expect("an instant within the range")
}

fn suite_credentials() -> Credentials<'static> {
    Credentials {
        access_key_id: "AKIDEXAMPLE",
        secret_access_key: SUITE_KEY,
        session_token: None,
    }
}

/// Signs a request of the AWS SigV4 test suite, which scopes to
/// `us-east-1/service` and sends to `example.amazonaws.com`.
fn suite_signed(method: &str, query: &str, headers: &[(&str, &str)], body: &[u8]) -> Signed {
    sign(&SignedRequest {
        credentials: suite_credentials(),
        scope: Scope {
            region: "us-east-1",
            service: "service",
        },
        timestamp: at(SUITE_DATE),
        method,
        host: "example.amazonaws.com",
        path: "/",
        query,
        headers,
        payload: Payload::Body(body),
        content_sha256: false,
        path_encoding: PathEncoding::Normalized,
    })
}

fn authorization(signed: &Signed) -> &str {
    signed
        .headers
        .iter()
        .find(|(name, _)| *name == "Authorization")
        .map_or("", |(_, value)| value.as_str())
}

fn names(signed: &Signed) -> Vec<&'static str> {
    signed.headers.iter().map(|(name, _)| *name).collect()
}

#[test]
fn sha256_hex_is_lowercase_hex() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn suite_get_vanilla() {
    let signed = suite_signed("GET", "", &[], b"");
    assert_eq!(
        signed.signature,
        "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    );
    assert_eq!(
        signed.headers,
        [
            ("X-Amz-Date", "20150830T123600Z".to_owned()),
            (
                "Authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=\
                 5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
                    .to_owned()
            ),
        ]
    );
}

#[test]
fn suite_get_vanilla_query_order_key() {
    let signed = suite_signed("GET", "Param2=value2&Param1=value1", &[], b"");
    assert_eq!(
        signed.signature,
        "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
    );
}

#[test]
fn suite_post_x_www_form_urlencoded() {
    let headers = [("Content-Type", "application/x-www-form-urlencoded")];
    let signed = suite_signed("POST", "", &headers, b"Param1=value1");
    assert_eq!(
        signed.signature,
        "ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a"
    );
}

/// What botocore's `SigV4Auth` (v2's vendored copy, run under Python with
/// `utcnow` fixed at 2026-11-21T12:00:00Z) puts in `Authorization` for an STS
/// AssumeRole form POST to `https://sts.us-east-1.amazonaws.com/` with a
/// session token: the token is set and signed as `add_auth` does.
#[test]
fn a_session_token_is_set_and_signed_as_botocore_does() {
    let body = b"Action=AssumeRole&RoleArn=arn%3Aaws%3Aiam%3A%3A123456789012%3Arole%2Fr\
&RoleSessionName=s%20%C3%A9&Version=2011-06-15";
    let signed = sign(&SignedRequest {
        credentials: Credentials {
            session_token: Some("tok123"),
            ..suite_credentials()
        },
        scope: Scope {
            region: "us-east-1",
            service: "sts",
        },
        timestamp: at(1_795_262_400),
        method: "POST",
        host: "sts.us-east-1.amazonaws.com",
        path: "/",
        query: "",
        headers: &[(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        )],
        payload: Payload::Body(body),
        content_sha256: false,
        path_encoding: PathEncoding::Normalized,
    });
    assert_eq!(
        names(&signed),
        ["X-Amz-Date", "X-Amz-Security-Token", "Authorization"]
    );
    assert_eq!(signed.headers[1].1, "tok123");
    assert_eq!(
        authorization(&signed),
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261121/us-east-1/sts/aws4_request, \
         SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, \
         Signature=e40aa30335b1831e35eeb557562704645b360a91ead31d163cdceb3fde5bea1b"
    );
}

/// `S3SigV4Auth` sends the payload's hash and signs it with the others;
/// headers botocore never signs are sent but left out of the signature.
#[test]
fn s3_sends_the_payload_hash_and_unsigned_headers_stay_out() {
    let with_agent = suite_signed("PUT", "", &[("User-Agent", "aws-cli/2")], b"x");
    let without = suite_signed("PUT", "", &[], b"x");
    assert_eq!(with_agent, without);
    let s3 = sign(&SignedRequest {
        credentials: suite_credentials(),
        scope: Scope {
            region: "us-east-1",
            service: "s3",
        },
        timestamp: at(SUITE_DATE),
        method: "GET",
        host: "bucket.s3.amazonaws.com",
        path: "/key",
        query: "",
        headers: &[],
        payload: Payload::Unsigned,
        content_sha256: true,
        path_encoding: PathEncoding::Raw,
    });
    assert_eq!(
        names(&s3),
        ["X-Amz-Date", "X-Amz-Content-SHA256", "Authorization"]
    );
    assert_eq!(s3.headers[1].1, UNSIGNED_PAYLOAD);
    assert!(
        authorization(&s3).contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date,"),
        "{s3:?}"
    );
}

/// The test servers' check accepts a request sent with exactly the headers
/// [`Signed::apply`] gives, and refuses one whose Host, token or body
/// differ. A stale token among the request's own headers is replaced.
#[test]
fn a_server_verifies_exactly_the_signed_headers() {
    let request = SignedRequest {
        credentials: Credentials {
            session_token: Some("tok"),
            ..suite_credentials()
        },
        scope: Scope {
            region: "us-east-1",
            service: "sts",
        },
        timestamp: at(SUITE_DATE),
        method: "POST",
        host: "127.0.0.1:8080",
        path: "/",
        query: "a=b",
        headers: &[
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("X-Amz-Security-Token", "stale"),
        ],
        payload: Payload::Body(b"Action=GetCallerIdentity"),
        content_sha256: false,
        path_encoding: PathEncoding::Normalized,
    };
    let head = |host: &str, signed: &Signed| {
        let mut head = format!("POST /?a=b HTTP/1.1\r\nHost: {host}\r\nUser-Agent: test\r\n");
        for (name, value) in signed.apply(request.headers) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head
    };
    let signed = sign(&request);
    let verify = |head: &str| {
        testing::verify(
            head,
            b"Action=GetCallerIdentity",
            SUITE_KEY,
            PathEncoding::Normalized,
        )
    };
    assert_eq!(verify(&head("127.0.0.1:8080", &signed)), Ok(()));
    assert!(verify(&head("sts.us-east-1.amazonaws.com", &signed)).is_err());
    let tokenless = Signed {
        headers: signed
            .headers
            .iter()
            .filter(|(name, _)| *name != "X-Amz-Security-Token")
            .cloned()
            .collect(),
        ..signed.clone()
    };
    assert!(verify(&head("127.0.0.1:8080", &tokenless)).is_err());
    let other_body = testing::verify(
        &head("127.0.0.1:8080", &signed),
        b"Action=AssumeRole",
        SUITE_KEY,
        PathEncoding::Normalized,
    );
    assert!(other_body.is_err());
}

/// A request signed again, a retry's, still carries what the last signing
/// set. botocore deletes `Authorization` and `X-Amz-Date` and, when it has
/// a token, `X-Amz-Security-Token` before it signs
/// (`SigV4Auth._modify_request_before_signing`), and `S3SigV4Auth` its
/// `X-Amz-Content-SHA256`: the signature is the one a fresh request gets,
/// and [`Signed::apply`] sends each of them once, with the new value.
#[test]
fn a_retry_s_stale_signing_headers_are_replaced_as_botocore_deletes_them() {
    let request = |headers| SignedRequest {
        credentials: Credentials {
            session_token: Some("new-token"),
            ..suite_credentials()
        },
        scope: Scope {
            region: "us-east-1",
            service: "s3",
        },
        timestamp: at(SUITE_DATE),
        method: "PUT",
        host: "bucket.s3.amazonaws.com",
        path: "/key",
        query: "",
        headers,
        payload: Payload::Body(b"body"),
        content_sha256: true,
        path_encoding: PathEncoding::Raw,
    };
    let fresh_base = [("Content-Type", "text/plain")];
    let stale_base = [
        ("Content-Type", "text/plain"),
        ("x-amz-date", "20000101T000000Z"),
        ("Authorization", "AWS4-HMAC-SHA256 Credential=OLD"),
        ("X-Amz-Security-Token", "old-token"),
        ("X-AMZ-CONTENT-SHA256", "UNSIGNED-PAYLOAD"),
    ];
    let fresh = sign(&request(&fresh_base));
    let retried = sign(&request(&stale_base));
    assert_eq!(retried, fresh);
    let sent = retried.apply(&stale_base);
    let mut expected = vec![("Content-Type", "text/plain")];
    expected.extend(
        fresh
            .headers
            .iter()
            .map(|(name, value)| (*name, value.as_str())),
    );
    assert_eq!(sent, expected);
    assert_eq!(sent.len(), 5, "{sent:?}");
}

/// Without a token botocore leaves a request's own
/// `X-Amz-Security-Token`, and signs it with the other headers.
#[test]
fn without_a_token_a_request_s_own_token_header_is_kept_and_signed() {
    let base = [("X-Amz-Security-Token", "callers")];
    let signed = suite_signed("GET", "", &base, b"");
    assert!(
        authorization(&signed).contains("SignedHeaders=host;x-amz-date;x-amz-security-token,"),
        "{signed:?}"
    );
    assert_eq!(
        signed.apply(&base),
        [
            ("X-Amz-Security-Token", "callers"),
            ("X-Amz-Date", "20150830T123600Z"),
            ("Authorization", authorization(&signed)),
        ]
    );
}

/// A request that already names its payload's hash is signed with that
/// hash, as `SigV4Auth.canonical_request` reads it from the header: the
/// same signature as asking the signer to send `UNSIGNED-PAYLOAD`.
#[test]
fn a_request_s_own_content_sha256_is_the_hash_it_is_signed_with() {
    let given = [("X-Amz-Content-SHA256", UNSIGNED_PAYLOAD)];
    let own = suite_signed("PUT", "", &given, b"ignored");
    let asked = sign(&SignedRequest {
        credentials: suite_credentials(),
        scope: Scope {
            region: "us-east-1",
            service: "service",
        },
        timestamp: at(SUITE_DATE),
        method: "PUT",
        host: "example.amazonaws.com",
        path: "/",
        query: "",
        headers: &[],
        payload: Payload::Unsigned,
        content_sha256: true,
        path_encoding: PathEncoding::Normalized,
    });
    assert_eq!(own.signature, asked.signature);
}

#[test]
fn header_values_collapse_whitespace_and_repeat_names_join() {
    let (canonical, signed) = canonical_headers(&[
        ("X-Amz-Meta", "  a   b "),
        ("x-amz-meta", "c"),
        ("Host", "h"),
    ]);
    assert_eq!(canonical, "host:h\nx-amz-meta:a b,c");
    assert_eq!(signed, "host;x-amz-meta");
}

#[test]
fn path_normalization_removes_dot_segments_and_requotes() {
    assert_eq!(normalize_path(""), "/");
    assert_eq!(normalize_path("/a/./b/../c"), "/a/c");
    assert_eq!(normalize_path("/a%20b"), "/a%2520b");
    assert_eq!(normalize_path("/~x/y z"), "/~x/y%20z");
}

/// Presigns with the access key and secret of the black-box suite, at the
/// date upstream freezes.
fn presigned(origin: &str, path: &str, expires: i64, token: Option<&str>) -> String {
    presign(&Presign {
        credentials: Credentials {
            access_key_id: "access_key",
            secret_access_key: "secret_key",
            session_token: token,
        },
        scope: Scope {
            region: "us-east-1",
            service: "s3",
        },
        timestamp: at(PRESIGN_DATE),
        method: "GET",
        origin,
        path,
        query: "",
        expires,
        path_encoding: PathEncoding::Raw,
    })
}

fn signature_of(url: &str) -> &str {
    url.rsplit_once("X-Amz-Signature=")
        .map_or("", |(_, tail)| tail)
}

#[test]
fn presign_vectors_at_the_frozen_date() {
    for (origin, path, expires, expected) in [
        (
            "https://bucket.s3.us-east-1.amazonaws.com",
            "/key",
            3600,
            "1297528058f2c8b89cfa52c6a47d6c54890700a1da24702b06d53e774c0acc95",
        ),
        (
            "https://s3.us-east-1.amazonaws.com",
            "/bucket.dots/key",
            3600,
            "5a032639cabfe3db0b4b87ba3b12c29f5e42fe74cbba8a0eb69bfb30c6e2d277",
        ),
        (
            "https://bucket.s3.us-east-1.amazonaws.com",
            "/key",
            1000,
            "865fb61b021c3bf406c40d41353f584835fff1f158cf1b3e6ec06260ecbb8937",
        ),
        (
            "https://s3.us-east-1.amazonaws.com",
            "/bucket/key",
            3600,
            "c6dab3560db76aded03e6268338ddb0a6dec00ebc82d6e7abdc305529fcaba74",
        ),
    ] {
        let url = presigned(origin, path, expires, None);
        assert_eq!(signature_of(&url), expected, "{url}");
    }
}

#[test]
fn presign_url_has_botocore_query_order() {
    let url = presigned(
        "https://bucket.s3.us-east-1.amazonaws.com",
        "/key",
        3600,
        None,
    );
    assert_eq!(
        url,
        "https://bucket.s3.us-east-1.amazonaws.com/key?X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Credential=access_key%2F20160818%2Fus-east-1%2Fs3%2Faws4_request\
         &X-Amz-Date=20160818T143303Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host\
         &X-Amz-Signature=1297528058f2c8b89cfa52c6a47d6c54890700a1da24702b06d53e774c0acc95"
    );
}

#[test]
fn presign_session_token_sits_before_the_signature() {
    let url = presigned(
        "https://bucket.s3.us-east-1.amazonaws.com",
        "/key",
        3600,
        Some("a+b/c=token"),
    );
    assert!(
        url.contains(
            "X-Amz-SignedHeaders=host&X-Amz-Security-Token=a%2Bb%2Fc%3Dtoken&X-Amz-Signature="
        ),
        "{url}"
    );
}

#[test]
fn presign_signs_the_default_port_free_lowercase_host() {
    let plain = presigned("http://Example.COM:80", "/k", 1, None);
    let lower = presigned("http://example.com", "/k", 1, None);
    assert_eq!(signature_of(&plain), signature_of(&lower));
    let other = presigned("http://example.com:8080", "/k", 1, None);
    assert_ne!(signature_of(&other), signature_of(&lower));
}

#[test]
fn host_header_follows_botocore() {
    assert_eq!(host_header("https", "Host.Example:443"), "host.example");
    assert_eq!(host_header("https", "host:8443"), "host:8443");
    assert_eq!(host_header("http", "user:pw@host:80"), "host");
    assert_eq!(host_header("http", "[::1]:9000"), "[::1]:9000");
    assert_eq!(host_header("https", "[::1]"), "[::1]");
}

#[test]
fn quote_matches_python() {
    assert_eq!(quote("a b/c~d", ""), "a%20b%2Fc~d");
    assert_eq!(quote("a b/c", "/"), "a%20b/c");
    assert_eq!(quote("café", "/"), "caf%C3%A9");
    assert_eq!(quote_bytes(b"k\xff /x", "/"), "k%FF%20/x");
}
