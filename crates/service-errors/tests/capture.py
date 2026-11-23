"""Captures the Python route's stderr and exit status for each case in
cases.json, as the expected bytes of the service-errors tests.

    python3 crates/service-errors/tests/capture.py \
        target/aarch64-apple-darwin/release-safe/aws

Each case runs the built binary with AWS_CLI_NATIVE_ROUTER=python, a
cleared environment and its own temporary HOME, config and credentials
files. A case's endpoint is a stub on loopback that this script serves,
or an address that refuses or never answers; nothing reaches a real
endpoint. The stub answers each request with the case's response, holds
the connection silent, or closes it without a byte.

It writes fixtures/<case>.stderr (the bytes, as written, with the run's
loopback port replaced by `{port}`) and fixtures/<case>.status (the exit
status). Stdout must be empty.
"""
import http.server
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
CONFIG = '[default]\nregion = us-west-2\n'
CREDENTIALS = (
    '[default]\naws_access_key_id = AKIDEXAMPLE\n'
    'aws_secret_access_key = wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY\n'
)


def stub(behaviour):
    """A server on loopback for one case, and its port."""

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *args):
            pass

        def do_POST(self):
            length = int(self.headers.get('Content-Length') or 0)
            self.rfile.read(length)
            kind = behaviour.get('kind', 'respond')
            if kind == 'silent':
                time.sleep(behaviour.get('seconds', 5))
                return
            if kind == 'close':
                self.connection.shutdown(socket.SHUT_RDWR)
                self.close_connection = True
                return
            body = behaviour.get('body', '').encode()
            self.send_response_only(behaviour['status'])
            for name, value in behaviour.get('headers', []):
                self.send_header(name, value)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        do_GET = do_POST

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def refused_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]


def substitute(text, port):
    return text.replace('{port}', str(port))


def capture(binary, case):
    server = stub(case['stub']) if 'stub' in case else None
    port = server.server_address[1] if server else refused_port()
    with tempfile.TemporaryDirectory() as home:
        config = os.path.join(home, 'config')
        credentials = os.path.join(home, 'credentials')
        with open(config, 'w') as f:
            f.write(case.get('config', CONFIG))
        with open(credentials, 'w') as f:
            f.write(case.get('credentials', CREDENTIALS))
        env = {
            'HOME': home,
            'PATH': '/usr/bin:/bin',
            'AWS_CONFIG_FILE': config,
            'AWS_SHARED_CREDENTIALS_FILE': credentials,
            'AWS_CLI_NATIVE_ROUTER': 'python',
            'AWS_MAX_ATTEMPTS': '1',
        }
        env.update({k: substitute(v, port) for k, v in case.get('env', {}).items()})
        argv = [binary] + [substitute(a, port) for a in case['argv']]
        done = subprocess.run(argv, env=env, capture_output=True, timeout=60)
    if server:
        server.shutdown()
    if done.stdout:
        raise SystemExit(f"{case['name']}: unexpected stdout {done.stdout!r}")
    # The port is the run's; the fixture names it as the case does.
    stderr = done.stderr.replace(f'127.0.0.1:{port}'.encode(), b'127.0.0.1:{port}')
    return stderr, done.returncode


def main():
    binary = os.path.abspath(sys.argv[1])
    only = set(sys.argv[2:])
    with open(os.path.join(HERE, 'cases.json')) as f:
        cases = json.load(f)
    for case in cases:
        if only and case['name'] not in only:
            continue
        stderr, status = capture(binary, case)
        base = os.path.join(HERE, 'fixtures', case['name'])
        with open(base + '.stderr', 'wb') as f:
            f.write(stderr)
        with open(base + '.status', 'w') as f:
            f.write(f'{status}\n')
        print(f"{case['name']}: {status} {stderr!r}")


main()
