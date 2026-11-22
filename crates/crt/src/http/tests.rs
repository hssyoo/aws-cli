//! The client against stand-in servers on loopback. Every timeout here is
//! short, and every wait on a server is bounded.
use super::{Error, Request, Response, Signing};
use crate::credentials::{Delegate, Query, Source, View};
use crate::s3::NO_CREDENTIALS_ERROR_CODE;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Long enough for loopback on a loaded machine, and short for a test.
const TIMEOUT: Duration = Duration::from_millis(300);
/// How long a test lets a server or a client take before it fails.
const PATIENCE: Duration = Duration::from_secs(20);

/// Answers every query from another thread, or fails it.
#[derive(Debug)]
struct Fixed {
    fail: bool,
}

impl Delegate for Fixed {
    fn credentials(&self, query: Query) {
        if self.fail {
            return query.fail();
        }
        std::thread::spawn(move || {
            query.complete(View {
                access_key_id: "AKIDDELEGATE",
                secret_access_key: "delegate-secret",
                session_token: Some("delegate-token"),
                account_id: None,
                expiration: None,
            });
        });
    }
}

fn runtime(fail: bool) -> Result<crate::Runtime, Box<crate::s3::TransferResult>> {
    crate::Runtime::new(crate::RuntimeOptions {
        // The S3 client is built but unused: the request goes around it.
        endpoint_url: "http://127.0.0.1:9".into(),
        credentials: Source::Delegate(Arc::new(Fixed { fail })),
        event_loop_count: 1,
        ..crate::RuntimeOptions::default()
    })
}

/// What a stand-in server does with the one connection it accepts, once
/// it has read the request's head.
#[derive(Clone, Copy)]
enum Script {
    /// Writes these bytes and closes.
    Answer(&'static [u8]),
    /// Writes nothing, and holds the connection until the test is done.
    Silent,
    /// Writes these bytes, then holds the connection until the test is
    /// done.
    Stall(&'static [u8]),
    /// Closes without a byte.
    Drop,
    /// Writes these bytes before it reads any, then reads until the
    /// client closes.
    Unasked(&'static [u8]),
}

/// A stand-in server for one connection. `done` releases a connection it
/// holds; the thread returns the request's head.
struct Server {
    port: u16,
    done: Sender<()>,
    thread: JoinHandle<std::io::Result<String>>,
}

impl Server {
    fn start(script: Script) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (done, held) = channel();
        let thread = std::thread::spawn(move || serve(&listener, script, &held));
        Ok(Self { port, done, thread })
    }
    /// The request's head, once the server has let go of its connection.
    fn finish(self) -> Result<String, Box<dyn std::error::Error>> {
        // The server may already be gone.
        let _ = self.done.send(());
        Ok(self.thread.join().map_err(|_| "the server panicked")??)
    }
}

fn serve(listener: &TcpListener, script: Script, held: &Receiver<()>) -> std::io::Result<String> {
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(PATIENCE))?;
    if let Script::Unasked(bytes) = script {
        stream.write_all(bytes)?;
    }
    let head = read_head(&mut stream)?;
    match script {
        Script::Answer(bytes) => stream.write_all(bytes)?,
        Script::Silent => hold(held),
        Script::Stall(bytes) => {
            stream.write_all(bytes)?;
            stream.flush()?;
            hold(held);
        }
        Script::Drop | Script::Unasked(_) => {}
    }
    Ok(head)
}

/// Keeps the connection open until the test lets go, or gives up.
fn hold(held: &Receiver<()>) {
    // A test that failed dropped its sender, which also ends the wait.
    let _ = held.recv_timeout(PATIENCE);
}

fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
    }
    Ok(String::from_utf8_lossy(&request).into_owned())
}

const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";

fn request<'a>(port: u16, headers: &'a [(&'a str, &'a str)]) -> Request<'a> {
    Request {
        host: "127.0.0.1",
        port,
        tls: false,
        method: "GET",
        target: "/2015-03-31/functions/my-fn/configuration",
        headers,
        body: &[],
        tunnel: None,
        signing: None,
        connect_timeout: TIMEOUT,
        read_timeout: TIMEOUT,
    }
}

#[test]
fn a_delegate_signs_a_request_outside_the_s3_client() -> TestResult {
    let server = Server::start(Script::Answer(OK))?;
    let runtime = runtime(false)?;
    let host = format!("127.0.0.1:{}", server.port);
    let headers = [("Host", host.as_str())];
    let signed = Request {
        signing: Some(Signing {
            region: "us-west-2",
            service: "lambda",
        }),
        ..request(server.port, &headers)
    };
    let response = runtime.http_client().send(&signed);
    assert_eq!(
        response,
        Ok(Response {
            status: 200,
            headers: vec![
                ("Content-Length".into(), "2".into()),
                ("Connection".into(), "close".into()),
            ],
            body: b"{}".to_vec()
        })
    );
    drop(runtime);
    let sent = server.finish()?;
    assert!(
        sent.starts_with("GET /2015-03-31/functions/my-fn/configuration HTTP/1.1\r\n"),
        "{sent}"
    );
    let lower = sent.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: aws4-hmac-sha256 credential=akiddelegate/"),
        "{sent}"
    );
    assert!(lower.contains("/us-west-2/lambda/aws4_request"), "{sent}");
    assert!(lower.contains("x-amz-security-token: delegate-token\r\n"));
    Ok(())
}

#[test]
fn a_failed_query_fails_the_request_before_it_connects() -> TestResult {
    let runtime = runtime(true)?;
    let signed = Request {
        signing: Some(Signing {
            region: "us-west-2",
            service: "lambda",
        }),
        ..request(9, &[])
    };
    assert_eq!(
        runtime.http_client().send(&signed),
        Err(Error::Connect(NO_CREDENTIALS_ERROR_CODE))
    );
    Ok(())
}

/// An [`crate::HttpRuntime`] starts only the stages its client connects
/// with: an endpoint no S3 client takes does not stop it, it sends an
/// unsigned request, and it refuses one that asks to be signed, before it
/// connects, without asking its delegate, which would have answered.
#[test]
fn an_http_runtime_sends_without_an_s3_client_and_signs_nothing() -> TestResult {
    let options = crate::RuntimeOptions {
        endpoint_url: String::new(),
        credentials: Source::Delegate(Arc::new(Fixed { fail: false })),
        event_loop_count: 1,
        ..crate::RuntimeOptions::default()
    };
    assert!(crate::Runtime::new(options.clone()).is_err(), "no endpoint");
    let runtime = crate::HttpRuntime::new_with_logger(options, crate::debug::Logger::new(false))?;
    let signed = Request {
        signing: Some(Signing {
            region: "us-west-2",
            service: "lambda",
        }),
        ..request(9, &[])
    };
    assert_eq!(
        runtime.http_client().send(&signed),
        Err(Error::Connect(NO_CREDENTIALS_ERROR_CODE))
    );
    let server = Server::start(Script::Answer(OK))?;
    let response = runtime
        .http_client()
        .send(&request(server.port, &[("Host", "127.0.0.1")]))?;
    assert_eq!(response.status, 200);
    drop(runtime);
    let sent = server.finish()?;
    assert!(
        !sent.to_ascii_lowercase().contains("authorization"),
        "{sent}"
    );
    Ok(())
}

/// The request goes out as it was given: unsigned, its headers in order,
/// an empty one among them, and nothing added.
#[test]
fn an_unsigned_request_is_sent_as_given_and_its_body_is_read() -> TestResult {
    let server = Server::start(Script::Answer(
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot there",
    ))?;
    let runtime = runtime(true)?;
    let headers = [
        ("Host", "metadata.example"),
        ("Accept-Encoding", "identity"),
        ("x-aws-ec2-metadata-token", ""),
        ("User-Agent", "aws-cli/2.0.0"),
        ("Content-Length", "0"),
    ];
    let put = Request {
        method: "PUT",
        target: "http://metadata.example/latest/api/token",
        ..request(server.port, &headers)
    };
    assert_eq!(
        runtime.http_client().send(&put),
        Ok(Response {
            status: 404,
            headers: vec![("Content-Length".into(), "9".into())],
            body: b"not there".to_vec()
        })
    );
    drop(runtime);
    assert_eq!(
        server.finish()?,
        "PUT http://metadata.example/latest/api/token HTTP/1.1\r\n\
         Host: metadata.example\r\n\
         Accept-Encoding: identity\r\n\
         x-aws-ec2-metadata-token: \r\n\
         User-Agent: aws-cli/2.0.0\r\n\
         Content-Length: 0\r\n\r\n"
    );
    Ok(())
}

#[test]
fn a_chunked_body_is_read_whole() -> TestResult {
    let server = Server::start(Script::Answer(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
          4\r\nrole\r\n5\r\n-name\r\n0\r\n\r\n",
    ))?;
    let runtime = runtime(true)?;
    let response = runtime.http_client().send(&request(server.port, &[]))?;
    assert_eq!(response.body, b"role-name");
    drop(runtime);
    server.finish()?;
    Ok(())
}

/// Nothing listens on the port once the listener is gone. Windows refuses
/// a loopback connect only after it has sent the SYN again, about two
/// seconds on, so the connect timeout is the long one.
#[test]
fn a_refused_connection_is_a_connect_error() -> TestResult {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let runtime = runtime(true)?;
    let refused = runtime.http_client().send(&Request {
        connect_timeout: PATIENCE,
        ..request(port, &[])
    });
    assert_eq!(
        refused,
        Err(Error::Connect(crt_sys::AWS_IO_SOCKET_CONNECTION_REFUSED))
    );
    Ok(())
}

/// A listener whose backlog is full leaves a connect unanswered, and the
/// CRT gives it up at the connect timeout.
#[test]
#[cfg(target_os = "linux")]
fn a_connect_that_is_not_answered_ends_at_the_connect_timeout() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    // Never accepted, these fill the queue of completed connections.
    let mut filling = Vec::new();
    for _ in 0..512 {
        let pending = TcpStream::connect_timeout(&address, Duration::from_millis(100));
        match pending {
            Ok(stream) => filling.push(stream),
            Err(_) => break,
        }
    }
    let runtime = runtime(true)?;
    let started = Instant::now();
    let unanswered = runtime.http_client().send(&request(address.port(), &[]));
    assert_eq!(
        unanswered,
        Err(Error::Connect(super::SOCKET_TIMEOUT_ERROR_CODE))
    );
    assert!(started.elapsed() < PATIENCE);
    Ok(())
}

/// The server never answers: the wait ends at the read timeout while the
/// server still holds the connection.
#[test]
fn a_response_that_never_starts_ends_at_the_read_timeout() -> TestResult {
    let server = Server::start(Script::Silent)?;
    let runtime = runtime(true)?;
    let started = Instant::now();
    let silent = runtime.http_client().send(&request(server.port, &[]));
    let waited = started.elapsed();
    assert_eq!(silent, Err(Error::ReadTimeout));
    assert!(waited >= TIMEOUT && waited < PATIENCE, "{waited:?}");
    drop(runtime);
    server.finish()?;
    Ok(())
}

/// The server sends its headers and part of the body, then stalls: the
/// call is abandoned at the read timeout, and the runtime's drop does not
/// wait for the server, which still holds the connection.
#[test]
fn a_stalled_body_is_abandoned_and_the_runtime_drops() -> TestResult {
    let server = Server::start(Script::Stall(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial",
    ))?;
    let runtime = runtime(true)?;
    let started = Instant::now();
    let stalled = runtime.http_client().send(&request(server.port, &[]));
    assert_eq!(stalled, Err(Error::ReadTimeout));
    drop(runtime);
    assert!(started.elapsed() < PATIENCE);
    server.finish()?;
    Ok(())
}

/// The server takes the connection and never answers the ClientHello.
/// The CRT gives the handshake up after the connect timeout, which bounds
/// it as well, while the server still holds the connection; the read
/// timeout, far longer here, has not begun.
#[test]
fn a_tls_handshake_that_stalls_ends_at_the_connect_timeout() -> TestResult {
    let server = Server::start(Script::Silent)?;
    let runtime = runtime(true)?;
    let started = Instant::now();
    let stalled = runtime.http_client().send(&Request {
        tls: true,
        read_timeout: PATIENCE,
        ..request(server.port, &[])
    });
    let waited = started.elapsed();
    assert_eq!(stalled, Err(Error::Connect(super::TLS_TIMEOUT_ERROR_CODE)));
    assert!(waited >= TIMEOUT && waited < PATIENCE, "{waited:?}");
    drop(runtime);
    assert!(started.elapsed() < PATIENCE);
    server.finish()?;
    Ok(())
}

/// A handshake that fails, here on the server's fatal alert, is a connect
/// error at once, not at the handshake's timeout, and no request is sent.
#[test]
fn a_tls_handshake_that_fails_is_a_connect_error() -> TestResult {
    // One TLS 1.2 alert record: fatal, handshake_failure.
    const ALERT: &[u8] = b"\x15\x03\x03\x00\x02\x02\x28";
    let server = Server::start(Script::Unasked(ALERT))?;
    let runtime = runtime(true)?;
    let failed = runtime.http_client().send(&Request {
        tls: true,
        connect_timeout: Duration::from_secs(5),
        ..request(server.port, &[])
    });
    assert_eq!(
        failed,
        Err(Error::Connect(
            crt_sys::AWS_IO_TLS_ERROR_NEGOTIATION_FAILURE
        ))
    );
    drop(runtime);
    assert!(!server.finish()?.contains("GET "));
    Ok(())
}

#[test]
fn a_connection_closed_without_a_response_is_a_response_error() -> TestResult {
    let server = Server::start(Script::Drop)?;
    let runtime = runtime(true)?;
    let dropped = runtime.http_client().send(&request(server.port, &[]));
    assert!(matches!(dropped, Err(Error::Response(_))), "{dropped:?}");
    drop(runtime);
    server.finish()?;
    Ok(())
}

/// A runtime dropped while a request waits closes the request's
/// connection, so neither the drop nor the request waits out the read
/// timeout, which here is longer than the test allows.
#[test]
fn a_runtime_drop_ends_a_request_in_flight() -> TestResult {
    let server = Server::start(Script::Silent)?;
    let runtime = runtime(true)?;
    let client = runtime.http_client();
    let port = server.port;
    let (sent, outcome) = channel();
    let sender = std::thread::spawn(move || {
        let slow = Request {
            read_timeout: PATIENCE * 10,
            ..request(port, &[])
        };
        // The receiver is there until the test has the outcome.
        let _ = sent.send(client.send(&slow));
    });
    // The request is in flight once the server has read it.
    let started = Instant::now();
    std::thread::sleep(TIMEOUT);
    drop(runtime);
    let ended = outcome.recv_timeout(PATIENCE)?;
    assert!(matches!(ended, Err(Error::Response(_))), "{ended:?}");
    assert!(started.elapsed() < PATIENCE);
    sender.join().map_err(|_| "the sender panicked")?;
    server.finish()?;
    Ok(())
}

/// aws-c-http will not send a header value that holds a control character,
/// and says so once the connection is up: nothing reaches the server.
#[test]
fn a_request_aws_c_http_refuses_is_not_sent() -> TestResult {
    let server = Server::start(Script::Drop)?;
    let runtime = runtime(true)?;
    let headers = [("x-aws-ec2-metadata-token", "to\u{1}ken")];
    let refused = runtime.http_client().send(&request(server.port, &headers));
    assert!(
        matches!(refused, Err(Error::Connect(code)) if super::is_refusal(code)),
        "{refused:?}"
    );
    drop(runtime);
    assert_eq!(server.finish()?, "");
    Ok(())
}

#[test]
fn a_client_that_outlives_its_runtime_is_closed() -> TestResult {
    let runtime = runtime(true)?;
    let client = runtime.http_client();
    drop(runtime);
    assert_eq!(client.send(&request(9, &[])), Err(Error::Closed));
    Ok(())
}

/// A header's value may be a session token and a target may name
/// credentials, so neither is in what a request shows of itself.
#[test]
fn a_request_s_debug_form_leaves_out_header_values_and_the_target() {
    let headers = [("x-aws-ec2-metadata-token", "MARKER-token")];
    let request = Request {
        target: "http://user:MARKER-password@metadata.example/latest/api/token",
        ..request(80, &headers)
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains("MARKER"), "{debug}");
    assert!(debug.contains("x-aws-ec2-metadata-token"), "{debug}");
    assert!(
        debug.contains("127.0.0.1") && debug.contains("GET"),
        "{debug}"
    );
}

/// A body may be a session token or a document of credentials.
#[test]
fn a_response_s_debug_form_leaves_out_the_body_and_header_values() {
    let response = Response {
        status: 200,
        headers: vec![("x-amz-security-token".into(), "MARKER-token".into())],
        body: br#"{"SecretAccessKey": "MARKER-secret"}"#.to_vec(),
    };
    let debug = format!("{response:?}");
    assert!(!debug.contains("MARKER"), "{debug}");
    assert!(debug.contains("200") && debug.contains("36"), "{debug}");
    assert!(debug.contains("x-amz-security-token"), "{debug}");
}

/// A response's headers come back in order, each name as it arrived and
/// each value read as `http.client` reads one, as Latin-1, and a name
/// that repeats stays repeated: the protocols read `x-amzn-RequestId` and
/// `x-amzn-ErrorType` from them.
#[test]
fn a_response_s_headers_are_returned_in_order_as_received() -> TestResult {
    let server = Server::start(Script::Answer(
        b"HTTP/1.1 400 Bad Request\r\n\
          x-amzn-RequestId: req-1\r\n\
          x-amzn-ErrorType: ValidationException:http://internal.amazon.com/\r\n\
          X-Dup: one\r\n\
          x-dup: two\r\n\
          X-Latin: caf\xe9\r\n\
          X-Empty:\r\n\
          Content-Length: 2\r\n\r\n{}",
    ))?;
    let runtime = runtime(true)?;
    let response = runtime.http_client().send(&request(server.port, &[]))?;
    drop(runtime);
    server.finish()?;
    assert_eq!(response.status, 400);
    let expected = [
        ("x-amzn-RequestId", "req-1"),
        (
            "x-amzn-ErrorType",
            "ValidationException:http://internal.amazon.com/",
        ),
        ("X-Dup", "one"),
        ("x-dup", "two"),
        ("X-Latin", "caf\u{e9}"),
        ("X-Empty", ""),
        ("Content-Length", "2"),
    ];
    let headers: Vec<(&str, &str)> = response
        .headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    assert_eq!(headers, expected);
    assert_eq!(response.body, b"{}");
    Ok(())
}

/// Serves one connection: reads the head, then as many body bytes as its
/// one `Content-Length` says (or, with none, whatever arrives in a short
/// while), answers [`OK`], and returns every byte it read.
fn serve_with_body(listener: &TcpListener) -> std::io::Result<Vec<u8>> {
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let mut raw = Vec::new();
    let mut chunk = [0; 4096];
    let mut want: Option<usize> = None;
    loop {
        let split = raw.windows(4).position(|window| window == b"\r\n\r\n");
        if let Some(split) = split {
            let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok());
            want = Some(split + 4 + length.unwrap_or(0));
        }
        if want.is_some_and(|want| raw.len() >= want) {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => raw.extend_from_slice(&chunk[..count]),
            // No framing and no more bytes: the body, if any, is read.
            Err(error) if want.is_some() => {
                let _ = error;
                break;
            }
            Err(error) => return Err(error),
        }
    }
    stream.write_all(OK)?;
    Ok(raw)
}

/// Sends `post` to a loopback server and returns its head and body.
fn exchange(
    post: &Request<'_>,
    server: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
    let runtime = runtime(true)?;
    let response = runtime.http_client().send(post)?;
    assert_eq!(response.status, 200);
    drop(runtime);
    let raw = server.join().map_err(|_| "the server panicked")??;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no head")?;
    Ok((
        String::from_utf8_lossy(&raw[..split]).into_owned(),
        raw[split + 4..].to_vec(),
    ))
}

fn content_lengths(head: &str) -> Vec<String> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().to_owned())
        .collect()
}

/// A body of every byte value, a NUL and non-UTF-8 among them, long
/// enough to cross several reads and writes.
#[test]
fn a_body_arrives_byte_exact_with_its_length() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || serve_with_body(&listener));
    let body: Vec<u8> = (0..300_000_u32).map(|index| (index % 256) as u8).collect();
    let host = format!("127.0.0.1:{port}");
    let headers = [
        ("Host", host.as_str()),
        ("Content-Type", "application/octet-stream"),
    ];
    let post = Request {
        method: "POST",
        target: "/",
        body: &body,
        ..request(port, &headers)
    };
    let (head, received) = exchange(&post, server)?;
    assert!(head.starts_with("POST / HTTP/1.1\r\n"), "{head}");
    assert_eq!(content_lengths(&head), ["300000"], "{head}");
    // The caller's headers keep their order.
    let host_at = head.find("Host: ").ok_or("no host")?;
    let type_at = head.find("Content-Type: ").ok_or("no type")?;
    assert!(host_at < type_at, "{head}");
    assert!(received == body, "{} bytes arrived", received.len());
    Ok(())
}

/// A query-protocol form body: percent escapes and raw UTF-8 stay as they
/// are given, and the target keeps its query.
#[test]
fn a_form_body_is_sent_as_given() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || serve_with_body(&listener));
    let body = "Action=AssumeRole&RoleSessionName=s%20%C3%A9&Raw=é&Empty=&Version=2011-06-15";
    let host = format!("127.0.0.1:{port}");
    let headers = [
        ("Host", host.as_str()),
        (
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        ),
    ];
    let post = Request {
        method: "POST",
        target: "/?a=b",
        body: body.as_bytes(),
        ..request(port, &headers)
    };
    let (head, received) = exchange(&post, server)?;
    assert!(head.starts_with("POST /?a=b HTTP/1.1\r\n"), "{head}");
    assert_eq!(content_lengths(&head), [body.len().to_string()], "{head}");
    assert_eq!(received, body.as_bytes());
    Ok(())
}

/// A caller that signs for itself sets `Authorization`, and the client,
/// sending unsigned, adds no credentials of its own to a request with a body.
#[test]
fn a_callers_authorization_is_sent_unchanged_and_nothing_is_signed_over_it() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || serve_with_body(&listener));
    let auth = "AWS4-HMAC-SHA256 Credential=AKIDCALLER/20261121/us-east-1/sts/aws4_request, \
                SignedHeaders=host;x-amz-date, Signature=00";
    let host = format!("127.0.0.1:{port}");
    let headers = [
        ("Host", host.as_str()),
        ("X-Amz-Date", "20261121T120000Z"),
        ("Authorization", auth),
    ];
    let post = Request {
        method: "POST",
        body: b"Action=GetCallerIdentity",
        ..request(port, &headers)
    };
    let (head, _) = exchange(&post, server)?;
    let lower = head.to_ascii_lowercase();
    assert_eq!(lower.matches("authorization:").count(), 1, "{head}");
    assert!(head.contains(&format!("Authorization: {auth}")), "{head}");
    assert!(!lower.contains("x-amz-security-token"), "{head}");
    assert_eq!(lower.matches("x-amz-date:").count(), 1, "{head}");
    Ok(())
}

/// The body may be a secret, so it is never shown.
#[test]
fn debug_output_never_shows_the_body() {
    let post = Request {
        method: "POST",
        body: b"SecretAccessKey=MARKER-secret",
        ..request(1, &[])
    };
    let debug = format!("{post:?}");
    assert!(
        !debug.contains("MARKER") && !debug.contains("Secret"),
        "{debug}"
    );
}

/// Sends `sent` to a loopback server that reads its body by its framing,
/// and returns the head that arrived.
fn framed_head(sent: Request<'_>) -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || serve_with_body(&listener));
    let (head, received) = exchange(&Request { port, ..sent }, server)?;
    assert_eq!(received, sent.body);
    Ok(head)
}

/// botocore frames every request but a GET, HEAD or OPTIONS, an empty one
/// with `Content-Length: 0` (`_prepare_headers`), and `http.client` frames
/// any body; a GET without one goes out unframed.
#[test]
fn requests_are_framed_as_botocore_and_http_client_frame_them() -> TestResult {
    for (method, body, framing) in [
        ("POST", &b""[..], &["0"][..]),
        ("PUT", b"", &["0"]),
        ("DELETE", b"", &["0"]),
        ("GET", b"", &[]),
        ("GET", b"q", &["1"]),
        ("POST", b"abc", &["3"]),
    ] {
        let head = framed_head(Request {
            method,
            body,
            ..request(0, &[("Host", "127.0.0.1")])
        })?;
        assert_eq!(content_lengths(&head), framing, "{method}: {head}");
        assert!(
            head.lines().last().is_some_and(|line| framing.is_empty()
                || line.eq_ignore_ascii_case(&format!("content-length: {}", framing[0]))),
            "the framing goes last: {head}"
        );
    }
    Ok(())
}

/// A `Content-Length` the caller gives is sent as given when it is the
/// body's, and otherwise refuses the request before anything is sent.
#[test]
fn a_callers_content_length_must_be_the_body_s() -> TestResult {
    let given = [("Host", "127.0.0.1"), ("content-length", "3")];
    let head = framed_head(Request {
        method: "POST",
        body: b"abc",
        ..request(0, &given)
    })?;
    assert_eq!(content_lengths(&head), ["3"], "{head}");
    let runtime = runtime(true)?;
    for (length, body) in [("5", &b"abc"[..]), ("0", b"abc"), ("1", b"")] {
        let wrong = [("Host", "127.0.0.1"), ("Content-Length", length)];
        let refused = runtime.http_client().send(&Request {
            method: "PUT",
            body,
            // Nothing listens there: a request that were sent would fail
            // to connect.
            ..request(1, &wrong)
        });
        assert_eq!(refused, Err(Error::Framing), "{length}");
    }
    Ok(())
}

/// The runtime signs a body it streams: it hashes the stream for
/// `x-amz-content-sha256`, then rewinds it, so the body still arrives whole
/// and the signature verifies over what arrived.
#[test]
fn a_signed_request_s_body_is_hashed_and_still_arrives_whole() -> TestResult {
    const BODY: &[u8] = b"Action=GetCallerIdentity&Version=2011-06-15";
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || serve_with_body(&listener));
    let host = format!("127.0.0.1:{port}");
    let headers = [
        ("Host", host.as_str()),
        ("Content-Type", "application/x-www-form-urlencoded"),
    ];
    let runtime = runtime(false)?;
    let response = runtime.http_client().send(&Request {
        method: "POST",
        target: "/",
        body: BODY,
        signing: Some(Signing {
            region: "us-east-1",
            service: "sts",
        }),
        ..request(port, &headers)
    })?;
    assert_eq!(response.status, 200);
    drop(runtime);
    let raw = server.join().map_err(|_| "the server panicked")??;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no head")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let body = &raw[split + 4..];
    assert_eq!(body, BODY);
    let lower = head.to_ascii_lowercase();
    assert!(
        lower.contains(&format!(
            "x-amz-content-sha256: {}\r\n",
            sigv4::sha256_hex(BODY)
        )),
        "{head}"
    );
    sigv4::testing::verify(&head, body, "delegate-secret", sigv4::PathEncoding::Raw)?;
    Ok(())
}

/// A stand-in proxy for one tunnel: reads the CONNECT, answers with
/// `reply`, then reads what the client sends through the tunnel until
/// `done` or a short while passes, and returns the CONNECT's head and those
/// bytes. With `reply` empty it answers nothing and holds the connection
/// until `done`.
fn proxy(
    listener: TcpListener,
    reply: &'static [u8],
    done: Receiver<()>,
) -> JoinHandle<std::io::Result<(String, Vec<u8>)>> {
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(PATIENCE))?;
        let head = read_head(&mut stream)?;
        if reply.is_empty() {
            hold(&done);
            return Ok((head, Vec::new()));
        }
        stream.write_all(reply)?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        let mut tunnelled = vec![0; 5];
        let count = stream.read(&mut tunnelled).unwrap_or(0);
        tunnelled.truncate(count);
        // One TLS 1.2 alert record: fatal, handshake_failure.
        let _ = stream.write_all(b"\x15\x03\x03\x00\x02\x02\x28");
        Ok((head, tunnelled))
    })
}

fn tunnelled(port: u16) -> Request<'static> {
    Request {
        host: "sts.example.invalid",
        port: 443,
        tls: true,
        tunnel: Some(super::Tunnel {
            host: "127.0.0.1",
            port,
        }),
        connect_timeout: Duration::from_secs(5),
        read_timeout: Duration::from_secs(5),
        ..request(0, &[("Host", "sts.example.invalid")])
    }
}

/// A TLS request through a tunnel: the proxy is asked to CONNECT to the
/// server's name and port, and the handshake then goes through it.
#[test]
fn a_tunnel_carries_the_handshake_through_the_proxy() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (_done, held) = channel();
    let proxy = proxy(
        listener,
        b"HTTP/1.1 200 Connection established\r\n\r\n",
        held,
    );
    let runtime = runtime(true)?;
    let failed = runtime.http_client().send(&tunnelled(port));
    assert!(matches!(failed, Err(Error::Connect(_))), "{failed:?}");
    drop(runtime);
    let (head, tunnelled) = proxy.join().map_err(|_| "the proxy panicked")??;
    assert!(
        head.starts_with("CONNECT sts.example.invalid:443 HTTP/1.1\r\n"),
        "{head}"
    );
    // A TLS handshake record: the ClientHello.
    assert_eq!(tunnelled.get(..2), Some(&b"\x16\x03"[..]), "{tunnelled:?}");
    Ok(())
}

/// A proxy that refuses the tunnel fails the request, and nothing else is
/// sent to it.
#[test]
fn a_refused_tunnel_fails_the_request() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (_done, held) = channel();
    let proxy = proxy(
        listener,
        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
        held,
    );
    let runtime = runtime(true)?;
    let failed = runtime.http_client().send(&tunnelled(port));
    assert!(matches!(failed, Err(Error::Connect(_))), "{failed:?}");
    drop(runtime);
    let (_, tunnelled) = proxy.join().map_err(|_| "the proxy panicked")??;
    assert_eq!(tunnelled, b"");
    Ok(())
}

/// A proxy that takes the connection and never answers CONNECT is given
/// up after the connect timeout: the request fails as a connect does, and
/// the runtime's release does not wait for the proxy, which still holds
/// its end.
#[test]
fn a_silent_proxy_is_given_up_after_the_connect_timeout() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (done, held) = channel();
    let proxy = proxy(listener, b"", held);
    let runtime = runtime(true)?;
    let started = Instant::now();
    let stalled = runtime.http_client().send(&Request {
        connect_timeout: TIMEOUT,
        read_timeout: PATIENCE,
        ..tunnelled(port)
    });
    let waited = started.elapsed();
    assert!(matches!(stalled, Err(Error::Connect(_))), "{stalled:?}");
    assert!(waited >= TIMEOUT && waited < PATIENCE, "{waited:?}");
    drop(runtime);
    assert!(started.elapsed() < PATIENCE, "{:?}", started.elapsed());
    let _ = done.send(());
    let (head, _) = proxy.join().map_err(|_| "the proxy panicked")??;
    assert!(
        head.starts_with("CONNECT sts.example.invalid:443 HTTP/1.1\r\n"),
        "{head}"
    );
    Ok(())
}

/// A caller that signs for itself, as the service client does: it signs
/// with `sigv4` over the Host it sends, and sends the request unsigned with
/// exactly the headers [`sigv4::Signed::apply`] gives, a stale token among
/// its own replaced. The server recomputes the signature from what arrived
/// and accepts it, and the client added only the body's framing.
#[test]
fn a_request_its_caller_signed_arrives_as_signed() -> TestResult {
    const BODY: &[u8] = b"Action=GetCallerIdentity&Version=2011-06-15";
    const SECRET: &str = "caller-secret";
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || serve_with_body(&listener));
    let host = format!("127.0.0.1:{port}");
    let base = [
        (
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        ),
        ("X-Amz-Security-Token", "stale"),
    ];
    let signed = sigv4::sign(&sigv4::SignedRequest {
        credentials: sigv4::Credentials {
            access_key_id: "AKIDCALLER",
            secret_access_key: SECRET,
            session_token: Some("caller-token"),
        },
        scope: sigv4::Scope {
            region: "us-east-1",
            service: "sts",
        },
        timestamp: timestamp::Timestamp::now(),
        method: "POST",
        host: &host,
        path: "/",
        query: "",
        headers: &base,
        payload: sigv4::Payload::Body(BODY),
        content_sha256: false,
        path_encoding: sigv4::PathEncoding::Normalized,
    });
    let mut headers = vec![("Host", host.as_str())];
    headers.extend(signed.apply(&base));
    let post = Request {
        method: "POST",
        target: "/",
        body: BODY,
        ..request(port, &headers)
    };
    let (head, body) = exchange(&post, server)?;
    assert_eq!(body, BODY);
    sigv4::testing::verify(&head, &body, SECRET, sigv4::PathEncoding::Normalized)?;
    let lower = head.to_ascii_lowercase();
    assert_eq!(lower.matches("x-amz-security-token:").count(), 1, "{head}");
    assert!(
        lower.contains("x-amz-security-token: caller-token\r\n"),
        "{head}"
    );
    let sent: Vec<&str> = head.lines().skip(1).collect();
    let mut expected: Vec<String> = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect();
    expected.push(format!("Content-Length: {}", BODY.len()));
    assert_eq!(sent, expected);
    Ok(())
}

/// A loopback HTTPS server on the shim's TLS (crt-sys `testing.c`), whose
/// certificate a CA made for the server issued to `sts.example.invalid`,
/// `localhost` and 127.0.0.1. It serves `connections` connections one at a
/// time, answering each whole request with `response`, and returns each
/// one's request, `None` for one whose handshake or exchange failed.
#[cfg(unix)]
struct TlsServer {
    port: u16,
    /// The CA's certificate, PEM.
    ca: Vec<u8>,
    thread: JoinHandle<Result<Vec<Option<Vec<u8>>>, String>>,
}

#[cfg(unix)]
impl TlsServer {
    fn start(
        response: &'static [u8],
        connections: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (ready, ca) = channel();
        let thread = std::thread::spawn(move || {
            let library = crate::runtime::Library::acquire();
            // SAFETY: no arguments; the library is held.
            let tls = unsafe { crt_sys::testing::zaws_test_tls_new() };
            let Some(tls) = std::ptr::NonNull::new(tls) else {
                let _ = ready.send(Vec::new());
                return Err("no TLS configuration".to_owned());
            };
            // SAFETY: the configuration is live, and the PEM is copied
            // before it is destroyed.
            let pem = unsafe { super::bytes(crt_sys::testing::zaws_test_tls_ca(tls.as_ptr())) };
            let _ = ready.send(pem.to_vec());
            let served = (0..connections)
                .map(|_| serve_tls(&listener, tls, response))
                .collect::<Result<Vec<_>, _>>();
            // SAFETY: every call that used it has returned.
            unsafe { crt_sys::testing::zaws_test_tls_destroy(tls.as_ptr()) };
            drop(library);
            served.map_err(|error| error.to_string())
        });
        let ca = ca.recv_timeout(PATIENCE)?;
        if ca.is_empty() {
            return Err("the TLS server did not start".into());
        }
        Ok(Self { port, ca, thread })
    }
    /// The requests of every connection, once the server served them all.
    fn finish(self) -> Result<Vec<Option<Vec<u8>>>, Box<dyn std::error::Error>> {
        Ok(self
            .thread
            .join()
            .map_err(|_| "the TLS server panicked")??)
    }
}

/// Accepts one connection on `listener` and serves it with `tls`.
#[cfg(unix)]
fn serve_tls(
    listener: &TcpListener,
    tls: std::ptr::NonNull<crt_sys::testing::TestTls>,
    response: &[u8],
) -> std::io::Result<Option<Vec<u8>>> {
    use std::os::fd::AsRawFd;
    let (stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(PATIENCE))?;
    stream.set_write_timeout(Some(PATIENCE))?;
    let mut request = vec![0; 1 << 16];
    let mut length = 0;
    // SAFETY: the configuration is live, the socket is open until `stream`
    // drops, and `request` holds the capacity given.
    let served = unsafe {
        crt_sys::testing::zaws_test_tls_serve(
            tls.as_ptr(),
            stream.as_raw_fd(),
            crt_sys::AwsByteCursor::from_bytes(response),
            request.as_mut_ptr(),
            request.len(),
            &mut length,
        )
    };
    request.truncate(length);
    Ok(served.then_some(request))
}

/// A stand-in HTTP proxy that tunnels: for each connection it reads the
/// CONNECT, answers 200, and relays bytes both ways to the server at a port
/// on loopback, whatever host the CONNECT named, until either side closes.
#[cfg(unix)]
struct TunnellingProxy {
    port: u16,
    done: Sender<()>,
    thread: JoinHandle<std::io::Result<Vec<String>>>,
}

#[cfg(unix)]
impl TunnellingProxy {
    /// Stops taking connections and returns the CONNECT heads it read.
    fn finish(self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let _ = self.done.send(());
        Ok(self.thread.join().map_err(|_| "the proxy panicked")??)
    }
}

/// A [`TunnellingProxy`] to the server at `port`.
#[cfg(unix)]
fn tunnelling_proxy(port: u16) -> std::io::Result<TunnellingProxy> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let proxy_port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let (done, stop) = channel::<()>();
    let thread = std::thread::spawn(move || {
        let mut heads = Vec::new();
        let started = Instant::now();
        while stop.try_recv().is_err() && started.elapsed() < PATIENCE {
            let mut client = match listener.accept() {
                Ok((client, _)) => client,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => return Err(error),
            };
            client.set_nonblocking(false)?;
            client.set_read_timeout(Some(PATIENCE))?;
            heads.push(read_head(&mut client)?);
            let server = TcpStream::connect(("127.0.0.1", port))?;
            client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
            relay(client, server)?;
        }
        Ok(heads)
    });
    Ok(TunnellingProxy {
        port: proxy_port,
        done,
        thread,
    })
}

/// Copies bytes each way between `client` and `server` on two threads,
/// each closing the other side's writes when its reads end.
#[cfg(unix)]
fn relay(client: TcpStream, server: TcpStream) -> std::io::Result<()> {
    server.set_read_timeout(Some(PATIENCE))?;
    let pairs = [(client.try_clone()?, server.try_clone()?), (server, client)];
    for (mut from, mut to) in pairs {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut from, &mut to);
            let _ = to.shutdown(std::net::Shutdown::Write);
        });
    }
    Ok(())
}

#[cfg(unix)]
fn http_runtime(
    trust: crate::runtime::Trust,
) -> Result<crate::HttpRuntime, Box<crate::s3::TransferResult>> {
    crate::HttpRuntime::new_with_logger(
        crate::RuntimeOptions {
            trust,
            verify_peer: true,
            event_loop_count: 1,
            ..crate::RuntimeOptions::default()
        },
        crate::debug::Logger::new(false),
    )
}

/// botocore's session for a call to an AWS service behind an HTTPS proxy
/// (`URLLib3Session` as a client builds it): a POST with a body to an
/// `https` endpoint goes through a CONNECT tunnel, the server's certificate
/// is verified with the CA bundle the runtime was given, and the request,
/// its body and the response with its headers cross the tunnel whole. The
/// platform's trust, which lacks the test CA, fails the same handshake.
#[test]
#[cfg(unix)]
fn a_post_through_a_proxy_tunnel_is_verified_with_the_given_ca_bundle() -> TestResult {
    const BODY: &[u8] = b"Action=GetCallerIdentity&Version=2011-06-15";
    let server = TlsServer::start(
        b"HTTP/1.1 200 OK\r\nx-amzn-RequestId: tls-1\r\nContent-Length: 2\r\n\r\n{}",
        2,
    )?;
    let proxy = tunnelling_proxy(server.port)?;
    let headers = [
        ("Host", "sts.example.invalid"),
        (
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        ),
    ];
    let post = Request {
        host: "sts.example.invalid",
        port: 443,
        tls: true,
        method: "POST",
        target: "/",
        body: BODY,
        tunnel: Some(super::Tunnel {
            host: "127.0.0.1",
            port: proxy.port,
        }),
        connect_timeout: Duration::from_secs(5),
        read_timeout: Duration::from_secs(5),
        ..request(0, &headers)
    };
    let runtime = http_runtime(crate::runtime::Trust::Pem(server.ca.clone()))?;
    let response = runtime.http_client().send(&post)?;
    drop(runtime);
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"{}");
    assert_eq!(
        response.headers.first(),
        Some(&("x-amzn-RequestId".to_owned(), "tls-1".to_owned()))
    );
    let untrusted = http_runtime(crate::runtime::Trust::Platform)?;
    let refused = untrusted.http_client().send(&post);
    drop(untrusted);
    assert!(matches!(refused, Err(Error::Connect(_))), "{refused:?}");
    let requests = server.finish()?;
    let expected = format!(
        "POST / HTTP/1.1\r\nHost: sts.example.invalid\r\n\
         Content-Type: application/x-www-form-urlencoded; charset=utf-8\r\n\
         Content-Length: {}\r\n\r\n",
        BODY.len()
    );
    let mut first = expected.into_bytes();
    first.extend_from_slice(BODY);
    assert_eq!(requests, [Some(first), None]);
    let heads = proxy.finish()?;
    assert_eq!(heads.len(), 2, "{heads:?}");
    for head in heads {
        assert!(
            head.starts_with("CONNECT sts.example.invalid:443 HTTP/1.1\r\n"),
            "{head}"
        );
    }
    Ok(())
}
