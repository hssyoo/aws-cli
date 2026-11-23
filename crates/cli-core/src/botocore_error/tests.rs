use super::*;

const URL: &str = "https://sts.us-east-1.amazonaws.com/";

/// Each exception's `fmt` (botocore/exceptions.py), as `str()` prints it.
#[test]
fn each_text_is_botocore_s() {
    for (text, expected) in [
        (
            endpoint_connection(URL),
            r#"Could not connect to the endpoint URL: "https://sts.us-east-1.amazonaws.com/""#,
        ),
        (
            connect_timeout(URL),
            r#"Connect timeout on endpoint URL: "https://sts.us-east-1.amazonaws.com/""#,
        ),
        (
            read_timeout(URL),
            r#"Read timeout on endpoint URL: "https://sts.us-east-1.amazonaws.com/""#,
        ),
        (
            connection_closed(URL),
            "Connection was closed before we received a valid response from endpoint URL: \
             \"https://sts.us-east-1.amazonaws.com/\".",
        ),
        (
            ssl_validation(URL, "[SSL: CERTIFICATE_VERIFY_FAILED]"),
            "SSL validation failed for https://sts.us-east-1.amazonaws.com/ \
             [SSL: CERTIFICATE_VERIFY_FAILED]",
        ),
        (
            http_client("boom"),
            "An HTTP Client raised an unhandled exception: boom",
        ),
    ] {
        assert_eq!(text, expected);
    }
    assert_eq!(
        proxy_connection(b"http://user:***@proxy:3128"),
        b"Failed to connect to proxy URL: \"http://user:***@proxy:3128\""
    );
    assert_eq!(NO_CREDENTIALS, "Unable to locate credentials");
    assert_eq!(NO_REGION, "You must specify a region.");
}

/// `ClientError.MSG_TEMPLATE` with `_get_retry_info`: the note only when
/// the attempts ran out; the bytes of code and message as given.
#[test]
fn a_client_error_reads_as_msg_template() {
    assert_eq!(
        client_error(b"AccessDenied", b"GetCallerIdentity", None, b"no"),
        b"An error occurred (AccessDenied) when calling the GetCallerIdentity operation: no"
    );
    assert_eq!(
        client_error(b"Throttling", b"ListTables", Some(2), b"slow\xff"),
        b"An error occurred (Throttling) when calling the ListTables operation \
          (reached max retries: 2): slow\xff"
    );
}
