//! Loopback servers for tests of the crates that send over [`crate::http`]
//! (feature `loopback`; Unix only, where the shim's test TLS exists): a TLS
//! server whose CA is made when it starts, and a proxy that tunnels.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a server waits for a client before it gives up.
const PATIENCE: Duration = Duration::from_secs(20);

/// A request's head, up to and with the blank line.
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

/// A loopback HTTPS server on the shim's TLS (crt-sys `testing.c`), whose
/// certificate a CA made for the server issued to `sts.example.invalid`,
/// `localhost` and 127.0.0.1. It serves `connections` connections one at a
/// time, answering each whole request with `response`, and returns each
/// one's request, `None` for one whose handshake or exchange failed.
#[derive(Debug)]
pub struct TlsServer {
    pub port: u16,
    /// The CA's certificate, PEM.
    pub ca: Vec<u8>,
    thread: JoinHandle<Result<Vec<Option<Vec<u8>>>, String>>,
}

impl TlsServer {
    pub fn start(
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
            let cursor = unsafe { crt_sys::testing::zaws_test_tls_ca(tls.as_ptr()) };
            // SAFETY: as above; a cursor of a live configuration's PEM.
            let pem = unsafe { std::slice::from_raw_parts(cursor.ptr, cursor.len) };
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
    pub fn finish(self) -> Result<Vec<Option<Vec<u8>>>, Box<dyn std::error::Error>> {
        Ok(self
            .thread
            .join()
            .map_err(|_| "the TLS server panicked")??)
    }
}

/// Accepts one connection on `listener` and serves it with `tls`.
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
#[derive(Debug)]
pub struct TunnellingProxy {
    pub port: u16,
    done: Sender<()>,
    thread: JoinHandle<std::io::Result<Vec<String>>>,
}

impl TunnellingProxy {
    /// Stops taking connections and returns the CONNECT heads it read.
    pub fn finish(self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let _ = self.done.send(());
        Ok(self.thread.join().map_err(|_| "the proxy panicked")??)
    }
}

/// A [`TunnellingProxy`] to the server at `port`.
pub fn tunnelling_proxy(port: u16) -> std::io::Result<TunnellingProxy> {
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
