"""A stand-in CLI for `cargo xtask diff --self-test`, which drives the
capture's paths the real CLI's happy path leaves alone (`standin.jsonl`):

- `sleep SECONDS`: outlives its timeout;
- `env`: prints its environment, a `NAME=value` line each, sorted;
- `get COUNT`: sends COUNT requests through the proxy (HTTPS_PROXY, trusting
  AWS_CA_BUNDLE), a connection each, printing each reply's status and body,
  or `closed` for a connection closed unanswered;
- `chunked`: sends through the proxy a body of two chunks, `hel` and `lo`;
- `malformed`: sends through the proxy a request the stub cannot parse;
- `uuid`: sends through the proxy a request whose `X-Token` header is a
  fresh version 4 UUID, as an idempotency token bound to a header is.

It uses the standard library alone, on the host's python3.
"""

from __future__ import annotations

import http.client
import os
import ssl
import sys
import time
import urllib.parse
import uuid
from typing import Final

HOST: Final = "standin.example.com"


def proxied() -> http.client.HTTPSConnection:
    proxy = urllib.parse.urlsplit(os.environ["HTTPS_PROXY"])
    context = ssl.create_default_context(cafile=os.environ["AWS_CA_BUNDLE"])
    connection = http.client.HTTPSConnection(
        proxy.hostname or "", proxy.port, context=context, timeout=10
    )
    connection.set_tunnel(HOST, 443)
    return connection


def get(count: int) -> None:
    for index in range(count):
        connection = proxied()
        try:
            connection.request("GET", f"/{index}")
            response = connection.getresponse()
            print(response.status, response.read().decode())
        except (ConnectionError, http.client.HTTPException):
            print("closed")
        finally:
            connection.close()


def chunked() -> None:
    connection = proxied()
    connection.request("PUT", "/chunked", body=iter([b"hel", b"lo"]), encode_chunked=True)
    print(connection.getresponse().status)
    connection.close()


def malformed() -> None:
    connection = proxied()
    connection.connect()
    if connection.sock is not None:
        connection.sock.sendall(b"GET /a\xe9 HTTP/1.1\r\nHost: " + HOST.encode() + b"\r\n\r\n")
        connection.sock.recv(1024)
    connection.close()


def token() -> None:
    connection = proxied()
    connection.request("POST", "/token", headers={"X-Token": str(uuid.uuid4())})
    print(connection.getresponse().status)
    connection.close()


def main() -> None:
    command, *rest = sys.argv[1:]
    if command == "sleep":
        time.sleep(float(rest[0]))
    elif command == "env":
        for name in sorted(os.environ):
            print(f"{name}={os.environ[name]}")
    elif command == "get":
        get(int(rest[0]))
    elif command == "chunked":
        chunked()
    elif command == "malformed":
        malformed()
    elif command == "uuid":
        token()
    else:
        raise SystemExit(f"standin: unknown command {command}")


if __name__ == "__main__":
    main()
