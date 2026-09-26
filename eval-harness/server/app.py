from __future__ import annotations

import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
from urllib.parse import parse_qs, urlsplit
import uuid

from harness.common import HarnessError, encoded, identifier, integer


class LocalServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, handler):
        self.slots = threading.BoundedSemaphore(32)
        super().__init__(address, handler)

    def get_request(self):
        sock, address = super().get_request()
        sock.settimeout(10)
        return sock, address

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, address)
        except Exception:
            self.slots.release()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()


def create_server(manager, port: int = 4318):
    token = uuid.uuid4().hex + uuid.uuid4().hex
    static = {
        "/": ("index.html", "text/html; charset=utf-8"),
        "/app.mjs": ("app.mjs", "text/javascript; charset=utf-8"),
        "/style.css": ("style.css", "text/css; charset=utf-8"),
    }

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def log_message(self, *args):
            pass  # Never log request bodies, credentials or arbitrary request paths.

        def send(self, status, value, kind="application/json; charset=utf-8", filename=None):
            raw = value if isinstance(value, bytes) else encoded(value)
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(raw)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")
            if filename:
                self.send_header("Content-Disposition", f'attachment; filename="{filename}"')
            self.end_headers()
            self.wfile.write(raw)

        def guard(self, api=True):
            allowed = {f"127.0.0.1:{self.server.server_port}", f"localhost:{self.server.server_port}"}
            if self.headers.get("Host", "") not in allowed:
                raise HarnessError("authorization", "Unexpected Host header")
            origin = self.headers.get("Origin")
            if origin is not None and origin not in {"http://" + host for host in allowed}:
                raise HarnessError("authorization", "Cross-origin requests are not allowed")
            if self.headers.get("Sec-Fetch-Site") not in (None, "same-origin", "none"):
                raise HarnessError("authorization", "Cross-site requests are not allowed")
            if api and not hmac.compare_digest(self.headers.get("X-Eval-Token", ""), token):
                raise HarnessError("authorization", "Refresh this local page to establish a session")

        def body(self):
            if self.headers.get("Content-Type", "").split(";")[0].strip() != "application/json" or self.headers.get("Transfer-Encoding"):
                raise HarnessError("invalid", "Content-Type application/json with Content-Length is required")
            try:
                length = int(self.headers.get("Content-Length", "0"))
            except ValueError:
                raise HarnessError("invalid", "Invalid Content-Length")
            if not 0 < length <= 65536:
                raise HarnessError("capacity", "Request body must be 1..65536 bytes")
            raw = self.rfile.read(length)
            if len(raw) != length:
                raise HarnessError("invalid", "Incomplete body")
            return json.loads(raw, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite")))

        def dispatch(self):
            parsed = urlsplit(self.path)
            path = parsed.path
            if self.command == "GET" and path in static:
                self.guard(api=False)
                file, kind = static[path]
                self.send(200, (manager.root / "web" / file).read_bytes(), kind)
                return
            if self.command == "GET" and path == "/api/bootstrap":
                self.guard(api=False)
                self.send(200, {"token": token, "version": "0.1.0"})
                return
            self.guard()
            query = parse_qs(parsed.query, strict_parsing=True) if parsed.query else {}

            def number(key, fallback, low, high):
                values = query.get(key, [str(fallback)])
                if len(values) != 1:
                    raise HarnessError("invalid", "Duplicate query parameter")
                try:
                    value = int(values[0])
                except ValueError:
                    raise HarnessError("invalid", "Invalid numeric query")
                return integer(value, key, low, high)

            if self.command == "GET" and path == "/api/catalog":
                self.send(200, manager.catalog.public())
                return
            if path == "/api/runs":
                if self.command == "GET":
                    self.send(200, manager.store.list(number("offset", 0, 0, 100000), number("limit", 50, 1, 100)))
                    return
                if self.command == "POST":
                    self.send(202, manager.start(self.body()))
                    return
            if self.command == "GET" and path == "/api/compare":
                self.send(200, manager.compare(identifier(query.get("left", [""])[0]), identifier(query.get("right", [""])[0])))
                return
            parts = path.strip("/").split("/")
            if len(parts) >= 3 and parts[:2] == ["api", "runs"]:
                rid = identifier(parts[2])
                if self.command == "GET" and len(parts) == 3:
                    report = manager.store.get(rid)
                    self.send(200, {key: value for key, value in report.items() if key != "manifest"})
                    return
                if self.command == "DELETE" and len(parts) == 3:
                    manager.delete(rid)
                    self.send(200, {"removed": rid})
                    return
                if self.command == "POST" and parts[3:] == ["cancel"]:
                    self.body()
                    self.send(200, manager.cancel(rid))
                    return
                if self.command == "GET" and parts[3:] == ["events"]:
                    self.send(200, manager.store.events(rid, number("after", 0, 0, 2**63 - 1)))
                    return
                if self.command == "GET" and parts[3:] == ["report"]:
                    self.send(200, manager.store.get(rid), filename=rid + ".json")
                    return
                if self.command == "GET" and len(parts) == 6 and parts[3] == "trials" and parts[5] == "evidence":
                    self.send(200, manager.evidence(rid, identifier(parts[4])), filename=rid + "-" + parts[4] + ".json")
                    return
            if self.command == "POST" and path == "/api/shutdown":
                self.body()
                self.send(200, {"stopping": True})
                threading.Thread(target=self.server.shutdown, daemon=True).start()
                return
            raise HarnessError("not_found", "Endpoint not found")

        def handle_call(self):
            try:
                try:
                    self.dispatch()
                except HarnessError as exc:
                    status = {"not_found": 404, "authorization": 403, "busy": 409, "conflict": 409, "capacity": 413, "integrity": 409}.get(exc.code, 400)
                    self.send(status, {"error": {"code": exc.code, "message": str(exc)}})
                except (ValueError, UnicodeError):
                    self.send(400, {"error": {"code": "invalid", "message": "Invalid JSON or query"}})
                except (BrokenPipeError, ConnectionResetError, TimeoutError):
                    pass
                except Exception:
                    self.send(500, {"error": {"code": "internal", "message": "Local operation failed; no success was assumed"}})
            except (BrokenPipeError, ConnectionResetError, TimeoutError):
                pass

        do_GET = handle_call
        do_POST = handle_call
        do_DELETE = handle_call

    return LocalServer(("127.0.0.1", port), Handler)
