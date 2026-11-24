"""The performance harness's loopback service: canned pages over HTTP/1.1.

One `Stub` listens on 127.0.0.1, over plain TCP or TLS, and answers the
n-th request of a command run with the n-th of the run's pages, so a
paginated command walks them in order. It honours keep-alive as an
HTTP/1.1 server does: a connection serves requests until the client
closes it or a request asks for `Connection: close` (or is HTTP/1.0).

Latency is injected where a network round trip would fall, with `rtt` as
the modelled round-trip time:

- connection setup: after `accept` and before the first byte is read (and
  before the TLS handshake), the stub waits one `rtt` for TCP's handshake
  and, over TLS, one more for TLS 1.3's, whatever version is negotiated.
  This is the cost a reused connection saves.
- each request: after its last byte is read and before the response's
  first byte is written, the stub waits one `rtt`.

So a fresh connection's first request costs 2 `rtt` over HTTP and 3 over
HTTPS, and every request on a reused connection costs 1.

The stub counts connections accepted, requests served and response bytes
written since the last `reset`.
"""

from __future__ import annotations

import socket
import ssl
import threading
import time
from dataclasses import dataclass, field
from typing import Final

# Requests from the CLI are small; a request head or body past this is
# answered with an error and its connection closed.
MAX_REQUEST_BYTES: Final = 1024 * 1024
RECEIVE_BYTES: Final = 64 * 1024
LISTEN_BACKLOG: Final = 128
# A delay spins this last stretch before its deadline (see `wait`).
SPIN_SECONDS: Final = 0.002


@dataclass
class Counts:
    """What one command run cost the service."""

    connections: int = 0
    requests: int = 0
    response_bytes: int = 0


@dataclass
class Script:
    """A run's pages, already framed, and the modelled round trip."""

    pages: list[bytes] = field(default_factory=list)
    rtt: float = 0.0


class RequestError(Exception):
    """A request the stub does not serve; its connection closes."""


class Stub:
    """A loopback service on its own port; `tls` makes it HTTPS."""

    def __init__(self, tls: ssl.SSLContext | None) -> None:
        self._tls = tls
        self._lock = threading.Lock()
        self._counts = Counts()
        self._script = Script()
        self._listener = socket.create_server(
            ("127.0.0.1", 0), backlog=LISTEN_BACKLOG
        )
        self.port: int = self._listener.getsockname()[1]
        self.url = f"{'https' if tls else 'http'}://127.0.0.1:{self.port}"
        self._thread = threading.Thread(target=self._accept, daemon=True)
        self._thread.start()

    def load(self, script: Script) -> None:
        """Serve `script` from the next request on, counting afresh."""
        with self._lock:
            self._script = script
            self._counts = Counts()

    def reset(self) -> None:
        """Start a new run: the first page next, and zero counts."""
        with self._lock:
            self._counts = Counts()

    def counts(self) -> Counts:
        with self._lock:
            return Counts(**vars(self._counts))

    def close(self) -> None:
        self._listener.close()

    def _accept(self) -> None:
        while True:
            try:
                connection, _ = self._listener.accept()
            except OSError:
                return
            with self._lock:
                self._counts.connections += 1
                rtt = self._script.rtt
            threading.Thread(
                target=self._serve, args=(connection, rtt), daemon=True
            ).start()

    def _serve(self, connection: socket.socket, rtt: float) -> None:
        with connection:
            connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            wait(rtt * (2 if self._tls else 1))
            try:
                stream = self._secure(connection)
                self._exchange(Reader(stream), stream, rtt)
            except (OSError, RequestError, ssl.SSLError):
                return

    def _secure(self, connection: socket.socket) -> socket.socket:
        if self._tls is None:
            return connection
        return self._tls.wrap_socket(connection, server_side=True)

    def _exchange(
        self, reader: Reader, stream: socket.socket, rtt: float
    ) -> None:
        while (request := reader.request()) is not None:
            wait(rtt)
            response = self._next_page(request.keep_alive)
            stream.sendall(response)
            with self._lock:
                self._counts.response_bytes += len(response)
            if not request.keep_alive:
                return

    def _next_page(self, keep_alive: bool) -> bytes:
        with self._lock:
            index = self._counts.requests
            self._counts.requests += 1
            pages = self._script.pages
        if index >= len(pages):
            return frame(500, "text/plain", b"no page left", keep_alive)
        page = pages[index]
        if keep_alive:
            return page
        return page.replace(
            b"Connection: keep-alive\r\n", b"Connection: close\r\n", 1
        )


@dataclass(frozen=True)
class Request:
    keep_alive: bool


class Reader:
    """Reads one HTTP/1.1 request at a time from a stream."""

    def __init__(self, stream: socket.socket) -> None:
        self._stream = stream
        self._buffer = b""

    def request(self) -> Request | None:
        """The next request, its body read and dropped; None at a clean
        end of stream."""
        head = self._head()
        if head is None:
            return None
        lines = head.decode("latin-1").split("\r\n")
        version = lines[0].rsplit(" ", 1)[-1]
        headers = {
            name.strip().lower(): value.strip()
            for name, _, value in (line.partition(":") for line in lines[1:])
        }
        if "chunked" in headers.get("transfer-encoding", "").lower():
            raise RequestError("chunked request bodies are not served")
        self._body(int(headers.get("content-length", "0")))
        token = headers.get("connection", "").lower()
        keep_alive = version == "HTTP/1.1" and token != "close"
        return Request(keep_alive=keep_alive)

    def _head(self) -> bytes | None:
        while b"\r\n\r\n" not in self._buffer:
            if len(self._buffer) > MAX_REQUEST_BYTES:
                raise RequestError("request head too large")
            chunk = self._stream.recv(RECEIVE_BYTES)
            if not chunk:
                if self._buffer:
                    raise RequestError("stream ended inside a request")
                return None
            self._buffer += chunk
        head, _, self._buffer = self._buffer.partition(b"\r\n\r\n")
        return head

    def _body(self, length: int) -> None:
        if length > MAX_REQUEST_BYTES:
            raise RequestError("request body too large")
        while len(self._buffer) < length:
            chunk = self._stream.recv(RECEIVE_BYTES)
            if not chunk:
                raise RequestError("stream ended inside a body")
            self._buffer += chunk
        self._buffer = self._buffer[length:]


def wait(seconds: float) -> None:
    """Wait `seconds` precisely. A timer may fire late by about half its
    interval (macOS's timer leeway, up to about 10 ms), so the wait sleeps
    in steps of half the time left before the last `SPIN_SECONDS`, which no
    late wake-up can carry past the deadline, and spins those."""
    deadline = time.perf_counter() + seconds
    while (left := deadline - time.perf_counter()) > SPIN_SECONDS:
        time.sleep((left - SPIN_SECONDS) / 2)
    while time.perf_counter() < deadline:
        pass


def frame(
    status: int,
    content_type: str,
    body: bytes,
    keep_alive: bool = True,
    headers: dict[str, str] | None = None,
) -> bytes:
    """A complete HTTP/1.1 response."""
    lines = [
        f"HTTP/1.1 {status} {'OK' if status == 200 else 'Error'}",
        f"Content-Type: {content_type}",
        f"Content-Length: {len(body)}",
        f"Connection: {'keep-alive' if keep_alive else 'close'}",
        "x-amzn-RequestId: 00000000-0000-4000-8000-000000000000",
        *(f"{name}: {value}" for name, value in (headers or {}).items()),
    ]
    return ("\r\n".join(lines) + "\r\n\r\n").encode("latin-1") + body
