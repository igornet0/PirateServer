#!/usr/bin/env python3
"""Tiny HTTP server for Docker protocol benchmarks: / and /index.html -> WIRE_UPSTREAM_OK; /size?bytes=N -> N bytes."""

from __future__ import annotations

import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

MAX_BYTES = 256 * 1024 * 1024
CHUNK = 64 * 1024


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802
        p = urlparse(self.path)
        if p.path in ("/", "/index.html"):
            body = b"WIRE_UPSTREAM_OK\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if p.path == "/stream":
            # Long-lived download for stability tests: /stream?seconds=N&chunk=65536
            qs = parse_qs(p.query)
            try:
                sec = float(qs.get("seconds", ["30"])[0])
            except ValueError:
                sec = 30.0
            try:
                chunk = int(qs.get("chunk", [str(CHUNK)])[0])
            except ValueError:
                chunk = CHUNK
            sec = max(0.1, min(sec, 600.0))
            chunk = max(1024, min(chunk, 1024 * 1024))
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            end = time.monotonic() + sec
            buf = b"\x5a" * chunk
            while time.monotonic() < end:
                self.wfile.write(f"{len(buf):x}\r\n".encode("ascii"))
                self.wfile.write(buf)
                self.wfile.write(b"\r\n")
                self.wfile.flush()
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
            return
        if p.path == "/size":
            qs = parse_qs(p.query)
            raw = qs.get("bytes", ["0"])[0]
            try:
                n = int(raw)
            except ValueError:
                n = 0
            n = max(0, min(n, MAX_BYTES))
            body = b"\x00" * n
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def do_POST(self) -> None:  # noqa: N802
        p = urlparse(self.path)
        if p.path == "/upload":
            raw_len = self.headers.get("Content-Length", "0")
            try:
                n = int(raw_len)
            except ValueError:
                n = 0
            n = max(0, min(n, MAX_BYTES))
            _ = self.rfile.read(n) if n > 0 else b""
            body = f'{{"received_bytes":{n}}}\n'.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def log_message(self, _format: str, *_args: object) -> None:
        return


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9000
    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
