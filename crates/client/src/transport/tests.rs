use super::*;
use crate::http::{Headers, HttpRequest, HttpResponse, Method, Url};
use crate::testing::InMemoryTransport;
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request(path: &str) -> Result<HttpRequest, &'static str> {
    Ok(HttpRequest {
        method: Method::Post,
        url: Url::parse(&format!("https://sts.example.invalid{path}")).ok_or("url")?,
        headers: Headers::from_pairs([("Content-Type", "application/x-www-form-urlencoded")]),
        body: b"Action=GetCallerIdentity".to_vec(),
    })
}

const TIMEOUTS: Timeouts = Timeouts {
    connect: Duration::from_secs(60),
    read: Duration::from_secs(60),
};

/// The in-memory transport answers each request with the next scripted
/// result, in order, records every request with its timeouts, and fails a
/// request it has no answer for.
#[test]
fn the_in_memory_transport_replays_its_script_and_records_requests() -> TestResult {
    let transport = InMemoryTransport::new();
    transport.respond(HttpResponse {
        status: 500,
        headers: Headers::new(),
        body: Vec::new(),
    });
    transport.fail(TransportError::ReadTimeout);
    transport.respond(HttpResponse {
        status: 200,
        headers: Headers::from_pairs([("x-amzn-RequestId", "r-1")]),
        body: b"<ok/>".to_vec(),
    });
    let first = transport.send(&request("/one")?, TIMEOUTS)?;
    assert_eq!(first.status, 500);
    assert_eq!(
        transport.send(&request("/two")?, TIMEOUTS),
        Err(TransportError::ReadTimeout)
    );
    let third = transport.send(&request("/three")?, TIMEOUTS)?;
    assert_eq!((third.status, third.body.as_slice()), (200, &b"<ok/>"[..]));
    assert!(matches!(
        transport.send(&request("/four")?, TIMEOUTS),
        Err(TransportError::Client { .. })
    ));
    let sent = transport.sent();
    let paths: Vec<&str> = sent
        .iter()
        .map(|(request, _)| request.url.path.as_str())
        .collect();
    assert_eq!(paths, ["/one", "/two", "/three", "/four"]);
    assert!(sent.iter().all(|(_, timeouts)| *timeouts == TIMEOUTS));
    assert_eq!(sent[0].0, request("/one")?);
    Ok(())
}

/// Each transport failure is the one of botocore's session it stands for,
/// and every one of them but an interrupt is one botocore retries
/// (`TransientRetryableChecker`: its `ConnectionError` and
/// `HTTPClientError` cover them all).
#[test]
fn every_transport_failure_but_an_interrupt_is_retryable() {
    for error in [
        TransportError::Connect,
        TransportError::ConnectTimeout,
        TransportError::ReadTimeout,
        TransportError::ConnectionClosed,
        TransportError::Tls {
            reason: "certificate verify failed".into(),
        },
        TransportError::Proxy {
            url: "http://proxy.example:3128".into(),
        },
        TransportError::Client {
            reason: "bad header".into(),
        },
    ] {
        assert!(error.is_retryable(), "{error:?}");
    }
    assert!(!TransportError::Interrupted.is_retryable());
}
