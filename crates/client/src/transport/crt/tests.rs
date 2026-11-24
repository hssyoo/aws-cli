//! The CRT transport against loopback servers: direct, through an HTTPS
//! proxy's tunnel with a CA bundle, and each failure as botocore's.
use super::*;
use crate::auth;
use crate::credentials::SigningCredentials;
use crate::http::{Headers, HttpRequest, Method, Url};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SECRET: &str = "transport-secret";
const BODY: &[u8] = b"Action=GetCallerIdentity&Version=2011-06-15";
const TIMEOUTS: Timeouts = Timeouts {
    connect: Duration::from_secs(5),
    read: Duration::from_secs(5),
};

fn env(pairs: &[(&str, &str)]) -> cli_core::SnapshotEnv {
    cli_core::SnapshotEnv::from_pairs(pairs.iter().map(|(name, value)| (*name, *value)))
}

fn transport(trust: ::crt::runtime::Trust, env: cli_core::SnapshotEnv) -> TestResult<CrtTransport> {
    Ok(CrtTransport::new(
        trust,
        false,
        env,
        ::crt::debug::Logger::new(false),
    )?)
}

/// An STS-shaped POST to `url`, signed for `us-east-1`.
fn signed_post(url: &str) -> TestResult<HttpRequest> {
    let mut request = HttpRequest {
        method: Method::Post,
        url: Url::parse(url).ok_or("url")?,
        headers: Headers::from_pairs([
            (
                "Content-Type",
                "application/x-www-form-urlencoded; charset=utf-8",
            ),
            ("User-Agent", "aws-cli/2.0.0"),
        ]),
        body: BODY.to_vec(),
    };
    let credentials = SigningCredentials {
        access_key_id: "AKIDTRANSPORT".into(),
        secret_access_key: SECRET.into(),
        session_token: Some("token".into()),
        account_id: None,
    };
    auth::sign(
        "us-east-1",
        "sts",
        &credentials,
        timestamp::Timestamp::now(),
        &mut request,
    );
    request
        .headers
        .set("Content-Length", BODY.len().to_string());
    Ok(request)
}

/// Serves one connection: reads the head and the body its Content-Length
/// names, answers `response`, and returns the head and the body.
fn serve_once(
    listener: TcpListener,
    response: &[u8],
) -> std::thread::JoinHandle<std::io::Result<(String, Vec<u8>)>> {
    let response = response.to_vec();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut raw = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&raw[..split + 2]).into_owned();
                let length = head
                    .to_ascii_lowercase()
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .map(|v| v.trim().to_owned())
                    })
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                if raw.len() >= split + 4 + length {
                    stream.write_all(&response)?;
                    return Ok((head, raw[split + 4..split + 4 + length].to_vec()));
                }
            }
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            raw.extend_from_slice(&chunk[..count]);
        }
    })
}

/// A signed POST goes out with the Host it was signed with, `http.client`'s
/// `Accept-Encoding`, and the request's headers in order; the server
/// recomputes the signature from what arrived and accepts it, and the
/// response comes back with its headers.
#[test]
fn a_signed_post_goes_direct_and_verifies() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = serve_once(
        listener,
        b"HTTP/1.1 200 OK\r\nx-amzn-RequestId: r-1\r\nContent-Length: 4\r\n\r\n<ok>",
    );
    let request = signed_post(&format!("http://127.0.0.1:{port}/"))?;
    let transport = transport(::crt::runtime::Trust::Platform, env(&[]))?;
    let response = transport.send(&request, TIMEOUTS)?;
    let (head, body) = server.join().map_err(|_| "server panicked")??;
    sigv4::testing::verify(&head, &body, SECRET, sigv4::PathEncoding::Normalized)?;
    let names: Vec<&str> = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(n, _)| n)
        .collect();
    assert_eq!(
        names,
        [
            "Host",
            "Accept-Encoding",
            "Content-Type",
            "User-Agent",
            "X-Amz-Date",
            "X-Amz-Security-Token",
            "Authorization",
            "Content-Length"
        ]
    );
    assert!(
        head.contains(&format!("Host: 127.0.0.1:{port}\r\n")),
        "{head}"
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.headers.get("x-amzn-requestid"), Some("r-1"));
    assert_eq!(response.body, b"<ok>");
    Ok(())
}

/// An STS-shaped POST through an HTTPS proxy: an `https` endpoint behind
/// `HTTPS_PROXY` goes through
/// the proxy's CONNECT tunnel, verified with the CA bundle the transport was
/// given, and arrives signed as sent.
#[test]
fn a_post_through_an_https_proxy_tunnel_uses_the_ca_bundle() -> TestResult {
    let server = ::crt::loopback::TlsServer::start(
        b"HTTP/1.1 200 OK\r\nx-amzn-RequestId: tls-1\r\nContent-Length: 2\r\n\r\n{}",
        1,
    )?;
    let proxy = ::crt::loopback::tunnelling_proxy(server.port)?;
    let proxy_url = format!("http://127.0.0.1:{}", proxy.port);
    let transport = transport(
        ::crt::runtime::Trust::Pem(server.ca.clone()),
        env(&[("HTTPS_PROXY", &proxy_url)]),
    )?;
    let request = signed_post("https://sts.example.invalid/")?;
    let response = transport.send(&request, TIMEOUTS)?;
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, &b"{}"[..])
    );
    let received = server.finish()?;
    let raw = received.first().cloned().flatten().ok_or("no request")?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no head")?;
    let head = String::from_utf8_lossy(&raw[..split + 2]).into_owned();
    sigv4::testing::verify(
        &head,
        &raw[split + 4..],
        SECRET,
        sigv4::PathEncoding::Normalized,
    )?;
    assert!(head.contains("Host: sts.example.invalid\r\n"), "{head}");
    let heads = proxy.finish()?;
    assert!(
        heads[0].starts_with("CONNECT sts.example.invalid:443 HTTP/1.1\r\n"),
        "{heads:?}"
    );
    Ok(())
}

/// Each failure is the one botocore's session raises: a refused connection
/// is `EndpointConnectionError`, a silent server `ReadTimeoutError`, an
/// unreachable proxy `ProxyConnectionError`, and a certificate the trust
/// does not verify `SSLError`.
#[test]
fn failures_map_to_botocore_s_transport_errors() -> TestResult {
    let refused = TcpListener::bind("127.0.0.1:0")?;
    let refused_port = refused.local_addr()?.port();
    drop(refused);
    let direct = transport(::crt::runtime::Trust::Platform, env(&[]))?;
    let send = |transport: &CrtTransport, url: &str| -> TestResult<Result<_, TransportError>> {
        let timeouts = Timeouts {
            connect: Duration::from_secs(2),
            read: Duration::from_millis(300),
        };
        Ok(transport.send(&signed_post(url)?, timeouts))
    };
    let refused = send(&direct, &format!("http://127.0.0.1:{refused_port}/"))?;
    assert_eq!(refused.err(), Some(TransportError::Connect));
    let silent = TcpListener::bind("127.0.0.1:0")?;
    let silent_port = silent.local_addr()?.port();
    let held = std::thread::spawn(move || {
        silent.accept().map(|(stream, _)| {
            std::thread::sleep(Duration::from_secs(2));
            drop(stream);
        })
    });
    let timed_out = send(&direct, &format!("http://127.0.0.1:{silent_port}/"))?;
    assert_eq!(timed_out.err(), Some(TransportError::ReadTimeout));
    let _ = held.join();
    let proxy_url = format!("http://127.0.0.1:{refused_port}");
    let proxied = transport(
        ::crt::runtime::Trust::Platform,
        env(&[("HTTP_PROXY", &proxy_url)]),
    )?;
    let unreachable = send(&proxied, "http://sts.example.invalid/")?;
    assert_eq!(
        unreachable.err(),
        Some(TransportError::Proxy { url: proxy_url })
    );
    let server =
        ::crt::loopback::TlsServer::start(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", 1)?;
    let untrusted = send(&direct, &format!("https://localhost:{}/", server.port))?;
    assert!(
        matches!(untrusted, Err(TransportError::Tls { .. })),
        "{untrusted:?}"
    );
    server.finish()?;
    Ok(())
}

/// `putrequest`'s defaults give way to the request's own headers of their
/// names, in any case (`skip_host`, `skip_accept_encoding`); the others
/// keep their place.
#[test]
fn a_request_s_own_host_or_accept_encoding_replaces_the_default() {
    let names = |own: &[(&str, &str)]| {
        let own = Headers::from_pairs(own.iter().copied());
        wire_headers("h.example", &own)
            .into_iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&[("User-Agent", "u")]),
        [
            "Host: h.example",
            "Accept-Encoding: identity",
            "User-Agent: u"
        ]
    );
    assert_eq!(
        names(&[("accept-encoding", "gzip"), ("User-Agent", "u")]),
        ["Host: h.example", "accept-encoding: gzip", "User-Agent: u"]
    );
    assert_eq!(
        names(&[("User-Agent", "u"), ("HOST", "other.example")]),
        [
            "Accept-Encoding: identity",
            "User-Agent: u",
            "HOST: other.example"
        ]
    );
}

/// `{"records": []}` as `gzip.compress(..., mtime=0)` writes it.
const GZIPPED: &[u8] = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\xab\x56\x2a\x4a\x4d\xce\
    \x2f\x4a\x29\x56\xb2\x52\x88\x8e\xad\x05\x00\x9a\x75\x09\xb7\x0f\x00\x00\x00";

/// neptunedata `GetPropertygraphStream --encoding gzip`: the member's
/// `Accept-Encoding` goes out alone, in the request's order, and the
/// signature over it verifies. The gzip answer comes back as it arrived,
/// `Content-Encoding` and all, as botocore reads it (`decode_content=False`).
#[test]
fn a_model_accept_encoding_is_sent_once_and_its_answer_is_not_decoded() -> TestResult {
    let head = b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 35\r\n\r\n";
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = serve_once(listener, &[&head[..], GZIPPED].concat());
    let mut request = HttpRequest {
        method: Method::Get,
        url: Url::parse(&format!("http://127.0.0.1:{port}/propertygraph/stream")).ok_or("url")?,
        headers: Headers::from_pairs([("Accept-Encoding", "gzip"), ("User-Agent", "aws-cli/2")]),
        body: Vec::new(),
    };
    let credentials = SigningCredentials {
        access_key_id: "AKIDTRANSPORT".into(),
        secret_access_key: SECRET.into(),
        session_token: None,
        account_id: None,
    };
    auth::sign(
        "us-east-1",
        "neptune-db",
        &credentials,
        timestamp::Timestamp::now(),
        &mut request,
    );
    let transport = transport(::crt::runtime::Trust::Platform, env(&[]))?;
    let response = transport.send(&request, TIMEOUTS)?;
    let (head, body) = server.join().map_err(|_| "server panicked")??;
    sigv4::testing::verify(&head, &body, SECRET, sigv4::PathEncoding::Normalized)?;
    let names: Vec<&str> = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        names,
        [
            "Host",
            "Accept-Encoding",
            "User-Agent",
            "X-Amz-Date",
            "Authorization"
        ]
    );
    assert!(head.contains("\r\nAccept-Encoding: gzip\r\n"), "{head}");
    assert_eq!(response.headers.get("content-encoding"), Some("gzip"));
    assert_eq!(response.body, GZIPPED);
    Ok(())
}

/// A loopback server that answers every request on a connection until the
/// connection has been idle for `idle`, then closes it, and counts the
/// connections it accepted.
fn keep_alive_server(
    idle: Duration,
) -> TestResult<(u16, std::sync::Arc<std::sync::atomic::AtomicUsize>)> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let accepted = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = accepted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || -> std::io::Result<()> {
                stream.set_read_timeout(Some(idle))?;
                let mut raw = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let length = String::from_utf8_lossy(&raw[..split])
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")?.trim().parse().ok()
                            })
                            .unwrap_or(0usize);
                        if raw.len() >= split + 4 + length {
                            raw.drain(..split + 4 + length);
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")?;
                            continue;
                        }
                    }
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return stream.shutdown(std::net::Shutdown::Both),
                        Ok(count) => raw.extend_from_slice(&chunk[..count]),
                    }
                }
            });
        }
    });
    Ok((port, accepted))
}

/// The pages of a paginated call reuse one connection, as botocore's pool
/// gives them. A server that closes the connection while it is idle
/// between two pages costs a new connection and nothing else: the next
/// request succeeds, so no error reaches the retry handler and no retry
/// quota is spent, as urllib3's check of a pooled connection before reuse
/// gives on the Python route.
#[test]
fn pages_share_a_connection_and_an_idle_close_costs_only_a_new_one() -> TestResult {
    use std::sync::atomic::Ordering;
    let idle = Duration::from_millis(100);
    let (port, accepted) = keep_alive_server(idle)?;
    let transport = transport(::crt::runtime::Trust::Platform, env(&[]))?;
    let url = format!("http://127.0.0.1:{port}/");
    for _ in 0..5 {
        let response = transport.send(&signed_post(&url)?, TIMEOUTS)?;
        assert_eq!(
            (response.status, response.body.as_slice()),
            (200, &b"{}"[..])
        );
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    std::thread::sleep(idle * 4);
    let response = transport.send(&signed_post(&url)?, TIMEOUTS)?;
    assert_eq!(response.status, 200);
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    Ok(())
}
