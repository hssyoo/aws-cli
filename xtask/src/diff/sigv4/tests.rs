use super::*;

/// The SigV4 test suite's example secret key.
const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn signed(
    method: &str,
    target: &str,
    host: &str,
    service: &str,
    signed: &str,
    signature: &str,
) -> Request {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/{service}/aws4_request, \
         SignedHeaders={signed}, Signature={signature}"
    );
    let mut headers = vec![
        ("Host".to_owned(), host.to_owned()),
        ("X-Amz-Date".to_owned(), "20150830T123600Z".to_owned()),
        ("Authorization".to_owned(), authorization),
    ];
    if service == "s3" {
        headers.push(("X-Amz-Content-SHA256".to_owned(), EMPTY_SHA256.to_owned()));
    }
    Request {
        method: method.to_owned(),
        target: target.to_owned(),
        version: "1.1".to_owned(),
        headers,
        body: Vec::new(),
        framing: None,
        complete: true,
    }
}

/// The test suite's get-vanilla, which botocore signs alike.
fn vanilla(signature: &str) -> Request {
    signed(
        "GET",
        "/",
        "example.amazonaws.com",
        "service",
        "host;x-amz-date",
        signature,
    )
}

const VANILLA: &str = "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31";

#[test]
fn a_signature_the_request_and_key_give_is_ok_and_a_tampered_one_bad() {
    assert_eq!(verify(&vanilla(VANILLA), Some(SECRET)), Verdict::Ok);
    let tampered = VANILLA.replacen('5', "6", 1);
    assert_eq!(verify(&vanilla(&tampered), Some(SECRET)), Verdict::Bad);
    assert_eq!(verify(&vanilla(VANILLA), Some("another")), Verdict::Bad);
    let mut moved = vanilla(VANILLA);
    moved.target = "/other".to_owned();
    assert_eq!(verify(&moved, Some(SECRET)), Verdict::Bad);
    let mut spaced = vanilla(VANILLA);
    spaced.headers[0].1 = "  example.amazonaws.com ".to_owned();
    assert_eq!(verify(&spaced, Some(SECRET)), Verdict::Ok);
}

#[test]
fn the_path_is_encoded_again_but_for_s3_and_the_query_sorted() {
    // Signatures botocore 's SigV4Auth and S3SigV4Auth give these requests.
    let post = signed(
        "POST",
        "/a%20b/c?b=2&a=1&a",
        "example.amazonaws.com",
        "service",
        "host;x-amz-date",
        "74b5f2e2562b6c9cc1ca95f13ba59b3e84130e0698b26f74aed24954b2709466",
    );
    let post = Request {
        body: b"{\"x\":1}".to_vec(),
        ..post
    };
    assert_eq!(verify(&post, Some(SECRET)), Verdict::Ok);
    let s3 = signed(
        "GET",
        "/a%20b?uploads",
        "bucket.s3.amazonaws.com",
        "s3",
        "host;x-amz-content-sha256;x-amz-date",
        "ff5f39daab294a67cf2da3755a23cfa6c4d89d47dac931e64f287a73bbe99044",
    );
    assert_eq!(verify(&s3, Some(SECRET)), Verdict::Ok);
    assert_eq!(encode("/a%20b/~x"), "/a%2520b/~x");
}

#[test]
fn what_cannot_be_checked_is_unverified_and_a_missing_header_bad() {
    assert_eq!(verify(&vanilla(VANILLA), None), Verdict::Unverified);
    let mut sigv4a = vanilla(VANILLA);
    sigv4a.headers[2].1 =
        sigv4a.headers[2]
            .1
            .replacen("AWS4-HMAC-SHA256", "AWS4-ECDSA-P256-SHA256", 1);
    assert_eq!(verify(&sigv4a, Some(SECRET)), Verdict::Unverified);
    let mut undated = vanilla(VANILLA);
    undated.headers.remove(1);
    assert_eq!(verify(&undated, Some(SECRET)), Verdict::Bad);
}

#[test]
fn hmac_matches_rfc_4231() {
    let mac = hmac(b"Jefe", b"what do ya want for nothing?");
    assert_eq!(
        hex(&mac),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    let long_key = [0xaa_u8; 131];
    let mac = hmac(
        &long_key,
        b"Test Using Larger Than Block-Size Key - Hash Key First",
    );
    assert_eq!(
        hex(&mac),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}
