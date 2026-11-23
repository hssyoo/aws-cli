use super::*;

fn service_error(code: Option<&str>, message: Option<&str>) -> ServiceError {
    let mut error = Map::new();
    for (key, text) in [("Code", code), ("Message", message)] {
        if let Some(text) = text {
            error.insert(key.into(), Value::String(text.into()));
        }
    }
    ServiceError {
        error,
        request_id: Some("req-1".into()),
        ..ServiceError::new("GetCallerIdentity", 403)
    }
}

/// `ClientError.MSG_TEMPLATE`, with `Unknown` for a code or message the
/// error lacks, and the retry note only when the attempts ran out.
#[test]
fn a_service_error_reads_as_botocore_s_client_error() {
    assert_eq!(
        service_error(Some("AccessDenied"), Some("not allowed")).to_string(),
        "An error occurred (AccessDenied) when calling the GetCallerIdentity operation: \
         not allowed"
    );
    assert_eq!(
        service_error(None, None).to_string(),
        "An error occurred (Unknown) when calling the GetCallerIdentity operation: Unknown"
    );
    let retried = ServiceError {
        retry_attempts: 2,
        ..service_error(Some("Throttling"), Some("slow down"))
    };
    assert_eq!(
        retried.to_string(),
        "An error occurred (Throttling) when calling the GetCallerIdentity operation: slow down"
    );
    let exhausted = ServiceError {
        max_attempts_reached: true,
        ..retried
    };
    assert_eq!(
        exhausted.to_string(),
        "An error occurred (Throttling) when calling the GetCallerIdentity operation \
         (reached max retries: 2): slow down"
    );
}

/// Each failure reads as the exception botocore raises for it.
#[test]
fn call_errors_read_as_botocore_s_exceptions() {
    let url = "https://sts.us-east-1.amazonaws.com/";
    let transport = |error| CallError::Transport {
        error,
        endpoint_url: url.into(),
    };
    for (error, text) in [
        (CallError::NoRegion, "You must specify a region."),
        (CallError::NoCredentials, "Unable to locate credentials"),
        (
            transport(TransportError::Connect),
            r#"Could not connect to the endpoint URL: "https://sts.us-east-1.amazonaws.com/""#,
        ),
        (
            transport(TransportError::ConnectTimeout),
            r#"Connect timeout on endpoint URL: "https://sts.us-east-1.amazonaws.com/""#,
        ),
        (
            transport(TransportError::ReadTimeout),
            r#"Read timeout on endpoint URL: "https://sts.us-east-1.amazonaws.com/""#,
        ),
        (
            transport(TransportError::ConnectionClosed),
            "Connection was closed before we received a valid response from endpoint URL: \
             \"https://sts.us-east-1.amazonaws.com/\".",
        ),
        (
            transport(TransportError::Tls {
                reason: "[SSL: CERTIFICATE_VERIFY_FAILED]".into(),
            }),
            "SSL validation failed for https://sts.us-east-1.amazonaws.com/ \
             [SSL: CERTIFICATE_VERIFY_FAILED]",
        ),
        (
            transport(TransportError::Proxy {
                url: "http://proxy.example:3128".into(),
            }),
            r#"Failed to connect to proxy URL: "http://proxy.example:3128""#,
        ),
        (
            transport(TransportError::Client {
                reason: "boom".into(),
            }),
            "An HTTP Client raised an unhandled exception: boom",
        ),
        (
            CallError::Endpoint("Invalid region: us-east-1!".into()),
            "Invalid region: us-east-1!",
        ),
        (
            CallError::Parse(ParseError::new("Unable to parse response")),
            "Unable to parse response",
        ),
        (
            CallError::Service(Box::new(service_error(Some("X"), Some("y")))),
            "An error occurred (X) when calling the GetCallerIdentity operation: y",
        ),
    ] {
        assert_eq!(error.to_string(), text);
    }
}

/// A validation failure reads as `ParamValidationError` does.
#[test]
fn a_validation_failure_reads_as_param_validation_error() {
    let error = CallError::Validation(model::validate::Report::default());
    assert_eq!(error.to_string(), "Parameter validation failed:\n");
}
