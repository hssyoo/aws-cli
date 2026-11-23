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

fn transport(trust: crt::runtime::Trust, env: cli_core::SnapshotEnv) -> TestResult<CrtTransport> {
    Ok(CrtTransport::new(trust, env, crt::debug::Logger::new(false))?)
}

/// An STS-shaped POST to `url`, signed for `us-east-1`.
fn signed_post(url: &str) -> TestResult<HttpRequest> {
    let mut request = HttpRequest {
        method: Method::Post,
        url: Url::parse(url).ok_or("url")?,
        headers: Headers::from_pairs([
            ("Content-Type", "application/x-www-form-urlencoded; charset=utf-8"),
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
    auth::sign("us-east-1", "sts", &credentials, timestamp::Timestamp::now(), &mut request);
    request.headers.set("Content-Length", BODY.len().to_string());
    Ok(request)
}

/// Serves one connection: reads the head and the body its Content-Length
/// names, answers `response`, and returns the head and the body.
fn serve_once(listener: TcpListener, response: &'static [u8]) -> std::thread::JoinHandle<std::io::Result<(String, Vec<u8>)>> {
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
                    .find_map(|line| line.strip_prefix("content-length:").map(|v| v.trim().to_owned()))
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                if raw.len() >= split + 4 + length {
                    stream.write_all(response)?;
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
    let transport = transport(crt::runtime::Trust::Platform, env(&[]))?;
    let response = transport.send(&request, TIMEOUTS)?;
    let (head, body) = server.join().map_err(|_| "server panicked")??;
    sigv4::testing::verify(&head, &body, SECRET, sigv4::PathEncoding::Normalized)?;
    let names: Vec<&str> = head.lines().skip(1).filter_map(|line| line.split_once(':')).map(|(n, _)| n).collect();
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
    assert!(head.contains(&format!("Host: 127.0.0.1:{port}\r\n")), "{head}");
    assert_eq!(response.status, 200);
    assert_eq!(response.headers.get("x-amzn-requestid"), Some("r-1"));
    assert_eq!(response.body, b"<ok>");
    Ok(())
}

/// M036's other half: an `https` endpoint behind `HTTPS_PROXY` goes through
/// the proxy's CONNECT tunnel, verified with the CA bundle the transport was
/// given, and arrives signed as sent.
#[test]
fn a_post_through_an_https_proxy_tunnel_uses_the_ca_bundle() -> TestResult {
    let server = crt::loopback::TlsServer::start(
        b"HTTP/1.1 200 OK\r\nx-amzn-RequestId: tls-1\r\nContent-Length: 2\r\n\r\n{}",
        1,
    )?;
    let proxy = crt::loopback::tunnelling_proxy(server.port)?;
    let proxy_url = format!("http://127.0.0.1:{}", proxy.port);
    let transport = transport(
        crt::runtime::Trust::Pem(server.ca.clone()),
        env(&[("HTTPS_PROXY", &proxy_url)]),
    )?;
    let request = signed_post("https://sts.example.invalid/")?;
    let response = transport.send(&request, TIMEOUTS)?;
    assert_eq!((response.status, response.body.as_slice()), (200, &b"{}"[..]));
    let received = server.finish()?;
    let raw = received.first().cloned().flatten().ok_or("no request")?;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no head")?;
    let head = String::from_utf8_lossy(&raw[..split + 2]).into_owned();
    sigv4::testing::verify(&head, &raw[split + 4..], SECRET, sigv4::PathEncoding::Normalized)?;
    assert!(head.contains("Host: sts.example.invalid\r\n"), "{head}");
    let heads = proxy.finish()?;
    assert!(heads[0].starts_with("CONNECT sts.example.invalid:443 HTTP/1.1\r\n"), "{heads:?}");
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
    let direct = transport(crt::runtime::Trust::Platform, env(&[]))?;
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
    let held = std::thread::spawn(move || silent.accept().map(|(stream, _)| {
        std::thread::sleep(Duration::from_secs(2));
        drop(stream);
    }));
    let timed_out = send(&direct, &format!("http://127.0.0.1:{silent_port}/"))?;
    assert_eq!(timed_out.err(), Some(TransportError::ReadTimeout));
    let _ = held.join();
    let proxy_url = format!("http://127.0.0.1:{refused_port}");
    let proxied = transport(crt::runtime::Trust::Platform, env(&[("HTTP_PROXY", &proxy_url)]))?;
    let unreachable = send(&proxied, "http://sts.example.invalid/")?;
    assert_eq!(unreachable.err(), Some(TransportError::Proxy { url: proxy_url }));
    let server = crt::loopback::TlsServer::start(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", 1)?;
    let untrusted = send(&direct, &format!("https://localhost:{}/", server.port))?;
    assert!(matches!(untrusted, Err(TransportError::Tls { .. })), "{untrusted:?}");
    server.finish()?;
    Ok(())
}
