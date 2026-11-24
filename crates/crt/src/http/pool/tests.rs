//! Pooled requests against stand-in servers on loopback that count the
//! connections they accept and the requests they answer.
use super::super::{Error, Request, Response, Tunnel};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// botocore's `max_pool_connections`.
const POOL: NonZeroU32 = NonZeroU32::new(10).unwrap();
/// How long a test lets a server or a client take before it fails.
const PATIENCE: Duration = Duration::from_secs(20);

/// What a stand-in server does after each response.
#[derive(Clone, Copy, Debug)]
enum After {
    /// Keeps the connection for the next request.
    KeepAlive,
    /// Answers with `Connection: close` and closes.
    Close,
    /// Answers as an HTTP/1.0 server without keep-alive, and closes.
    Http10,
    /// Closes the connection once it has been idle this long.
    IdleClose(Duration),
}

/// A loopback HTTP server that serves each connection on a thread of its
/// own and counts what it accepted and answered.
struct Server {
    port: u16,
    accepted: Arc<AtomicUsize>,
    answered: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(after: After) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let answered = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (accepted, answered, stop) = (accepted.clone(), answered.clone(), stop.clone());
            std::thread::spawn(move || {
                let started = Instant::now();
                while !stop.load(Ordering::SeqCst) && started.elapsed() < PATIENCE {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            accepted.fetch_add(1, Ordering::SeqCst);
                            let answered = answered.clone();
                            std::thread::spawn(move || serve(stream, after, &answered));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        Ok(Self {
            port,
            accepted,
            answered,
            stop,
            thread: Some(thread),
        })
    }
    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
    fn answered(&self) -> usize {
        self.answered.load(Ordering::SeqCst)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Answers each request on `stream` as `after` says, until the client
/// closes or the server does.
fn serve(mut stream: TcpStream, after: After, answered: &AtomicUsize) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(PATIENCE));
    let idle = match after {
        After::IdleClose(idle) => idle,
        _ => PATIENCE,
    };
    let _ = stream.set_read_timeout(Some(idle));
    loop {
        if !read_request(&mut stream) {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        let response: &[u8] = match after {
            After::KeepAlive | After::IdleClose(_) => {
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"
            }
            After::Close => b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok",
            After::Http10 => b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok",
        };
        if stream.write_all(response).is_err() {
            return;
        }
        answered.fetch_add(1, Ordering::SeqCst);
        if matches!(after, After::Close | After::Http10) {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    }
}

/// Reads one request's head; these requests have no body. False when the
/// client closed, or nothing came within the read timeout.
fn read_head_of(stream: &mut TcpStream, head: &mut Vec<u8>) -> bool {
    let mut chunk = [0; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return false,
            Ok(count) => head.extend_from_slice(&chunk[..count]),
        }
    }
    true
}

fn read_request(stream: &mut TcpStream) -> bool {
    read_head_of(stream, &mut Vec::new())
}

fn get(port: u16) -> Request<'static> {
    Request {
        host: "127.0.0.1",
        port,
        tls: false,
        method: "GET",
        target: "/",
        headers: &[("Host", "127.0.0.1")],
        body: b"",
        tunnel: None,
        signing: None,
        keep_alive: false,
        connect_timeout: Duration::from_secs(5),
        read_timeout: Duration::from_secs(5),
    }
}

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

fn ok(response: Result<Response, Error>) -> Result<(), Box<dyn std::error::Error>> {
    let response = response?;
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, &b"ok"[..])
    );
    Ok(())
}

/// Sequential requests to one server share one connection, as urllib3's
/// pool hands back the connection the last response released.
#[test]
fn sequential_requests_reuse_one_connection() -> TestResult {
    let server = Server::start(After::KeepAlive)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for _ in 0..5 {
        ok(client.send_pooled(&get(server.port), POOL))?;
    }
    assert_eq!((server.accepted(), server.answered()), (1, 5));
    Ok(())
}

/// Without a pool each request has a connection of its own, as before.
#[test]
fn unpooled_requests_connect_each_time() -> TestResult {
    let server = Server::start(After::KeepAlive)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for _ in 0..3 {
        ok(client.send(&get(server.port)))?;
    }
    assert_eq!(server.accepted(), 3);
    Ok(())
}

/// Each server has its own pool: requests that alternate between two keep
/// one connection to each.
#[test]
fn two_servers_keep_a_connection_each() -> TestResult {
    let first = Server::start(After::KeepAlive)?;
    let second = Server::start(After::KeepAlive)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for _ in 0..3 {
        ok(client.send_pooled(&get(first.port), POOL))?;
        ok(client.send_pooled(&get(second.port), POOL))?;
    }
    assert_eq!((first.accepted(), second.accepted()), (1, 1));
    assert_eq!((first.answered(), second.answered()), (3, 3));
    Ok(())
}

/// A client keeps ten pools, urllib3's `num_pools`: an eleventh server
/// drops the least recently used pool, whose server then gets a new
/// connection.
#[test]
fn an_eleventh_server_drops_the_least_recently_used_pool() -> TestResult {
    let servers = (0..11)
        .map(|_| Server::start(After::KeepAlive))
        .collect::<std::io::Result<Vec<_>>>()?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for server in &servers {
        ok(client.send_pooled(&get(server.port), POOL))?;
    }
    ok(client.send_pooled(&get(servers[1].port), POOL))?;
    ok(client.send_pooled(&get(servers[0].port), POOL))?;
    assert_eq!((servers[0].accepted(), servers[1].accepted()), (2, 1));
    Ok(())
}

/// A server that closes an idle connection between two requests costs a
/// new connection and nothing else: the second request succeeds, so no
/// error reaches the caller and nothing is retried, as urllib3's
/// `is_connection_dropped` check before reuse gives.
#[test]
fn a_connection_the_server_closed_while_idle_is_replaced_without_an_error() -> TestResult {
    let idle = Duration::from_millis(50);
    let server = Server::start(After::IdleClose(idle))?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for round in 1..=3 {
        ok(client.send_pooled(&get(server.port), POOL))?;
        assert_eq!(server.accepted(), round);
        std::thread::sleep(idle * 4);
    }
    assert_eq!(server.answered(), 3);
    Ok(())
}

/// A response with `Connection: close` ends its connection: the next
/// request connects anew.
#[test]
fn a_connection_close_response_is_not_reused() -> TestResult {
    let server = Server::start(After::Close)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for _ in 0..3 {
        ok(client.send_pooled(&get(server.port), POOL))?;
    }
    assert_eq!((server.accepted(), server.answered()), (3, 3));
    Ok(())
}

/// An HTTP/1.0 server that closes after its response, as such servers do,
/// is not reused, however soon the next request follows.
#[test]
fn an_http_1_0_server_that_closes_is_not_reused() -> TestResult {
    let server = Server::start(After::Http10)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for _ in 0..3 {
        ok(client.send_pooled(&get(server.port), POOL))?;
    }
    assert_eq!((server.accepted(), server.answered()), (3, 3));
    Ok(())
}

/// A request that fails does not give its connection back: the next one
/// connects anew.
#[test]
fn a_failed_response_closes_its_connection() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || -> std::io::Result<usize> {
        let mut connections = 0;
        for answer in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort"[..],
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
        ] {
            let (mut stream, _) = listener.accept()?;
            connections += 1;
            stream.set_read_timeout(Some(PATIENCE))?;
            read_request(&mut stream);
            stream.write_all(answer)?;
            stream.shutdown(Shutdown::Write)?;
            let _ = stream.read(&mut [0; 16]);
        }
        Ok(connections)
    });
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    let failed = client.send_pooled(&get(port), POOL);
    assert!(matches!(failed, Err(Error::Response(_))), "{failed:?}");
    ok(client.send_pooled(&get(port), POOL))?;
    drop(runtime);
    assert_eq!(server.join().map_err(|_| "the server panicked")??, 2);
    Ok(())
}

/// Requests through a tunnel keep the tunnel: one CONNECT carries them all.
#[test]
#[cfg(unix)]
fn a_tunnel_is_kept_for_its_server() -> TestResult {
    let server = Server::start(After::KeepAlive)?;
    let proxy = crate::loopback::tunnelling_proxy(server.port)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    let tunnelled = Request {
        tunnel: Some(Tunnel {
            host: "127.0.0.1",
            port: proxy.port,
        }),
        ..get(server.port)
    };
    for _ in 0..3 {
        ok(client.send_pooled(&tunnelled, POOL))?;
    }
    drop(runtime);
    let heads = proxy.finish()?;
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert_eq!((server.accepted(), server.answered()), (1, 3));
    Ok(())
}

/// Over TLS a kept connection saves the handshake too: three requests, one
/// connection, one handshake.
#[test]
#[cfg(unix)]
fn a_tls_connection_is_kept() -> TestResult {
    let server =
        crate::loopback::TlsServer::start(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", 1)?;
    let runtime = http_runtime(crate::runtime::Trust::Pem(server.ca.clone()))?;
    let client = runtime.http_client();
    let secure = Request {
        host: "localhost",
        tls: true,
        headers: &[("Host", "localhost")],
        ..get(server.port)
    };
    for _ in 0..3 {
        ok(client.send_pooled(&secure, POOL))?;
    }
    drop(runtime);
    let served = server.finish_counted()?;
    assert_eq!(served.len(), 1);
    assert_eq!(served[0].1, 3);
    Ok(())
}
