use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn names(headers: &Headers) -> Vec<&str> {
    headers.iter().map(|(name, _)| name).collect()
}

/// botocore's request headers keep the order names were first set; setting
/// a name again, in any case, replaces its value in place and keeps the
/// spelling it was first set with.
#[test]
fn headers_keep_first_set_order_and_replace_in_place() {
    let mut headers = Headers::new();
    headers.set("X-Amz-Target", "DynamoDB_20120810.GetItem");
    headers.set("Content-Type", "application/x-amz-json-1.0");
    headers.set("x-amz-target", "DynamoDB_20120810.PutItem");
    assert_eq!(names(&headers), ["X-Amz-Target", "Content-Type"]);
    assert_eq!(
        headers.get("X-AMZ-TARGET"),
        Some("DynamoDB_20120810.PutItem")
    );
    assert_eq!(headers.len(), 2);
}

/// A response may repeat a name; lookup is case-insensitive and finds the
/// first, `get_all` finds every one in order, and removal takes them all.
#[test]
fn repeated_names_are_kept_found_and_removed_together() {
    let mut headers =
        Headers::from_pairs([("x-amzn-RequestId", "a"), ("X-Dup", "1"), ("x-dup", "2")]);
    headers.append("X-DUP", "3");
    assert_eq!(headers.get("x-dup"), Some("1"));
    assert_eq!(
        headers.get_all("X-Dup").collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
    assert!(headers.contains("X-AMZN-REQUESTID"));
    headers.remove("x-DUP");
    assert_eq!(names(&headers), ["x-amzn-RequestId"]);
    assert_eq!(headers.get("missing"), None);
    assert!(!headers.is_empty());
}

/// Values may be tokens and bodies may be credentials: neither is printed.
#[test]
fn debug_output_shows_no_header_value_or_body() -> TestResult {
    let mut headers = Headers::new();
    headers.set("X-Amz-Security-Token", "MARKER-token");
    let request = HttpRequest {
        method: Method::Post,
        url: Url::parse("https://sts.example.invalid/?X-Amz-Signature=MARKER-sig")
            .ok_or("unparsed")?,
        headers: headers.clone(),
        body: b"SecretAccessKey=MARKER-secret".to_vec(),
    };
    let response = HttpResponse {
        status: 200,
        headers,
        body: b"MARKER-body".to_vec(),
    };
    for debug in [format!("{request:?}"), format!("{response:?}")] {
        assert!(!debug.contains("MARKER"), "{debug}");
        assert!(debug.contains("X-Amz-Security-Token"), "{debug}");
    }
    Ok(())
}

#[test]
fn methods_are_the_model_s_spellings() {
    for (text, method) in [
        ("GET", Method::Get),
        ("POST", Method::Post),
        ("PUT", Method::Put),
        ("DELETE", Method::Delete),
        ("HEAD", Method::Head),
        ("PATCH", Method::Patch),
        ("OPTIONS", Method::Options),
    ] {
        assert_eq!(Method::parse(text), Some(method));
        assert_eq!(method.as_str(), text);
    }
    assert_eq!(Method::parse("get"), None);
    assert_eq!(Method::parse("TRACE"), None);
}

/// A URL is split into what the pipeline joins and signs, and written back
/// as it was given.
#[test]
fn urls_split_into_scheme_authority_path_and_query() -> TestResult {
    for (text, scheme, authority, path, query) in [
        (
            "https://sts.us-east-1.amazonaws.com",
            "https",
            "sts.us-east-1.amazonaws.com",
            "/",
            "",
        ),
        (
            "http://127.0.0.1:8080/a/b?x=1&y",
            "http",
            "127.0.0.1:8080",
            "/a/b",
            "x=1&y",
        ),
        (
            "https://[::1]:443/?Action=Get",
            "https",
            "[::1]:443",
            "/",
            "Action=Get",
        ),
    ] {
        let url = Url::parse(text).ok_or(text)?;
        assert_eq!(
            (
                url.scheme.as_str(),
                url.authority.as_str(),
                url.path.as_str(),
                url.query.as_str()
            ),
            (scheme, authority, path, query),
            "{text}"
        );
    }
    let written = Url::parse("http://127.0.0.1:8080/a?b=c").map(|url| url.to_string());
    assert_eq!(written.as_deref(), Some("http://127.0.0.1:8080/a?b=c"));
    for invalid in ["", "sts.amazonaws.com", "https://", "://host"] {
        assert_eq!(Url::parse(invalid), None, "{invalid}");
    }
    Ok(())
}
