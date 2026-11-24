//! Pooled requests against stand-in servers on loopback that count the
//! connections they accept and the requests they answer.
use super::super::{Error, Request, Response, Tunnel};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const POOL: NonZeroU32 = super::POOL_CONNECTIONS;
/// How long a test lets a server or a client take before it fails.
const PATIENCE: Duration = Duration::from_secs(20);

/// What a stand-in server does with each connection.
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
    /// After its first response, writes bytes nobody asked for and keeps
    /// the connection open.
    SendGarbage,
    /// Answers the first request; reads the second's head and closes
    /// without answering.
    CloseAfterSecondHead,
    /// On the server's first connection, reads the request and never
    /// answers, until the client closes; later connections keep alive.
    StallFirst,
    /// Closes each connection as soon as it is accepted.
    CloseAtOnce,
}

/// A loopback HTTP server that serves each connection on a thread of its
/// own and counts what it accepted, answered and closed.
struct Server {
    port: u16,
    accepted: Arc<AtomicUsize>,
    answered: Arc<AtomicUsize>,
    closed: Arc<(Mutex<usize>, Condvar)>,
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
        let closed = Arc::new((Mutex::new(0), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (accepted, answered, closed, stop) = (
                accepted.clone(),
                answered.clone(),
                closed.clone(),
                stop.clone(),
            );
            std::thread::spawn(move || {
                let started = Instant::now();
                while !stop.load(Ordering::SeqCst) && started.elapsed() < PATIENCE {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let index = accepted.fetch_add(1, Ordering::SeqCst);
                            let (answered, closed) = (answered.clone(), closed.clone());
                            std::thread::spawn(move || {
                                serve(stream, after, index, &answered);
                                let (count, changed) = &*closed;
                                *count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
                                changed.notify_all();
                            });
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
            closed,
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
    /// Waits until the server has closed `count` connections.
    fn wait_closed(&self, count: usize) -> Result<(), &'static str> {
        let (closed, changed) = &*self.closed;
        let guard = closed.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = changed
            .wait_timeout_while(guard, PATIENCE, |closed| *closed < count)
            .unwrap_or_else(PoisonError::into_inner);
        if *guard < count {
            return Err("the server did not close the connection");
        }
        Ok(())
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

const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

/// Answers each request on `stream`, the server's `index`th connection, as
/// `after` says, until the client closes or the server does.
fn serve(mut stream: TcpStream, after: After, index: usize, answered: &AtomicUsize) {
    let idle = match after {
        After::IdleClose(idle) => idle,
        _ => PATIENCE,
    };
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(PATIENCE));
    let _ = stream.set_read_timeout(Some(idle));
    let mut request = 0;
    while !matches!(after, After::CloseAtOnce) && read_request(&mut stream) {
        let Some(response) = answer(after, index, request) else {
            if matches!(after, After::StallFirst) {
                // Holds the request until the client gives up and closes.
                let _ = stream.read(&mut [0; 16]);
            }
            break;
        };
        if stream.write_all(response).is_err() {
            return;
        }
        answered.fetch_add(1, Ordering::SeqCst);
        if !keep(&mut stream, after, request) {
            break;
        }
        request += 1;
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// What follows the `request`th response: whether the connection stays open
/// for another, after any unasked bytes `after` sends.
fn keep(stream: &mut TcpStream, after: After, request: usize) -> bool {
    match after {
        After::Close | After::Http10 => false,
        After::SendGarbage if request == 0 => {
            let _ = stream.write_all(b"garbage nobody asked for");
            true
        }
        _ => true,
    }
}

/// The response to the `request`th request on the server's `index`th
/// connection, or `None` to answer nothing and close.
fn answer(after: After, index: usize, request: usize) -> Option<&'static [u8]> {
    match after {
        After::StallFirst if index == 0 => None,
        After::CloseAfterSecondHead if request == 1 => None,
        After::Close => {
            Some(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok")
        }
        After::Http10 => Some(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok"),
        _ => Some(OK),
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
/// new connection and nothing else: the next request succeeds, so no
/// error reaches the caller and nothing is retried, as urllib3's
/// `is_connection_dropped` check before reuse gives.
#[test]
fn a_connection_the_server_closed_while_idle_is_replaced_without_an_error() -> TestResult {
    let server = Server::start(After::IdleClose(Duration::from_millis(50)))?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    for round in 1..=3 {
        ok(client.send_pooled(&get(server.port), POOL))?;
        assert_eq!(server.accepted(), round);
        server.wait_closed(round)?;
    }
    assert_eq!(server.answered(), 3);
    Ok(())
}

/// A kept connection on which the server sent something unasked is not
/// reused: the check before reuse finds it readable, and the next request
/// gets a new connection and succeeds.
#[test]
fn a_connection_with_unasked_data_waiting_is_not_reused() -> TestResult {
    let server = Server::start(After::SendGarbage)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    ok(client.send_pooled(&get(server.port), POOL))?;
    // Long enough for the bytes to arrive; whether the connection notices
    // them first or the check does, it is not reused.
    std::thread::sleep(Duration::from_millis(50));
    ok(client.send_pooled(&get(server.port), POOL))?;
    assert_eq!((server.accepted(), server.answered()), (2, 2));
    Ok(())
}

/// A kept connection the server closes after the check, once it has the
/// request, fails that request as the connection ending before its
/// response (`Error::Response`, which botocore raises as
/// `ConnectionClosedError` and retries); the next request connects anew.
#[test]
fn a_kept_connection_closed_after_the_request_fails_it_as_closed() -> TestResult {
    let server = Server::start(After::CloseAfterSecondHead)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    ok(client.send_pooled(&get(server.port), POOL))?;
    let failed = client.send_pooled(&get(server.port), POOL);
    assert!(matches!(failed, Err(Error::Response(_))), "{failed:?}");
    ok(client.send_pooled(&get(server.port), POOL))?;
    assert_eq!(server.accepted(), 2);
    Ok(())
}

/// A pooled request that times out closes its connection instead of giving
/// it back: the next request connects anew.
#[test]
fn a_timed_out_request_does_not_give_its_connection_back() -> TestResult {
    let server = Server::start(After::StallFirst)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    let stalled = Request {
        read_timeout: Duration::from_millis(200),
        ..get(server.port)
    };
    let failed = client.send_pooled(&stalled, POOL);
    assert!(matches!(failed, Err(Error::ReadTimeout)), "{failed:?}");
    ok(client.send_pooled(&get(server.port), POOL))?;
    assert_eq!((server.accepted(), server.answered()), (2, 1));
    Ok(())
}

/// A connection opened for the request is used unchecked, as urllib3 uses
/// a new connection: one the server closes at once fails the request, and
/// the client does not open connection after connection.
#[test]
fn a_new_connection_is_used_unchecked() -> TestResult {
    let server = Server::start(After::CloseAtOnce)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    let failed = client.send_pooled(&get(server.port), POOL);
    assert!(failed.is_err(), "{failed:?}");
    drop(runtime);
    assert_eq!(server.accepted(), 1);
    Ok(())
}

/// Pooled requests on one client go one at a time: two threads sending at
/// once to a server both get their answer, over the one connection.
#[test]
fn pooled_requests_on_one_client_go_one_at_a_time() -> TestResult {
    let server = Server::start(After::KeepAlive)?;
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let client = runtime.http_client();
    let port = server.port;
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let client = client.clone();
            std::thread::spawn(move || client.send_pooled(&get(port), POOL).map(|r| r.status))
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().map_err(|_| "a sender panicked")??, 200);
    }
    assert_eq!((server.accepted(), server.answered()), (1, 2));
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

/// A body whose `Content-Length` is known arrives in a buffer reserved for
/// it once, so it never grows by doubling, which would make the call's
/// peak memory depend on whether each growth happens in place.
#[test]
fn a_body_of_known_length_is_read_into_one_reservation() -> TestResult {
    const LENGTH: usize = 3 * 1024 * 1024 + 17;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(PATIENCE))?;
        read_request(&mut stream);
        stream
            .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {LENGTH}\r\n\r\n").as_bytes())?;
        stream.write_all(&vec![b'x'; LENGTH])?;
        let _ = stream.read(&mut [0; 16]);
        Ok(())
    });
    let runtime = http_runtime(crate::runtime::Trust::Platform)?;
    let response = runtime.http_client().send_pooled(&get(port), POOL)?;
    drop(runtime);
    server.join().map_err(|_| "the server panicked")??;
    assert_eq!(response.body.len(), LENGTH);
    assert_eq!(response.body.capacity(), LENGTH);
    Ok(())
}

/// The reservation is the declared length, up to a bound past which the
/// body grows as it arrives: a length nothing has sent yet allocates no
/// more than that.
#[test]
fn a_declared_length_is_reserved_up_to_a_bound() {
    use super::super::body_reservation;
    assert_eq!(body_reservation("4388904"), 4_388_904);
    assert_eq!(body_reservation(" 12 "), 12);
    assert_eq!(
        body_reservation("99999999999999"),
        super::super::BODY_RESERVATION_MAX
    );
    assert_eq!(body_reservation("-1"), 0);
    assert_eq!(body_reservation("ten"), 0);
}
