"""Transparent model transport observer/admission gate. No prompt edits or retries."""
from __future__ import annotations

import hashlib
import hmac
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import socket
import threading
import time
from urllib.parse import urlsplit
import uuid

from .common import HarnessError, Control, digest, encoded


class ModelGateway:
    def __init__(self, model: dict, limits: dict, control: Control, emit):
        self.model = model
        self.limits = limits
        self.control = control
        self.emit = emit
        self.token = uuid.uuid4().hex + uuid.uuid4().hex
        self.lock = threading.Lock()
        self.calls: list[dict] = []
        self.active: set = set()
        self.blocked = None
        self.closed = threading.Event()
        owner = self
        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.0"
            def log_message(self, *args): pass
            def do_POST(self): owner.handle(self)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        port = self.server.server_address[1]
        suffix = {"chat_completions": "chat/completions", "responses": "responses", "messages": "messages"}[model["protocol"]]
        self.path = "/v1/" + suffix
        self.endpoint = f"http://127.0.0.1:{port}{self.path}"
        self.thread = threading.Thread(target=lambda: self.server.serve_forever(poll_interval=0.05), daemon=True)
        self.thread.start()
        self.watcher = threading.Thread(target=self.watch, daemon=True)
        self.watcher.start()

    def watch(self):
        while not self.closed.wait(0.1):
            if self.control.cancel.is_set() or time.monotonic() >= self.control.deadline:
                self.disconnect()
                return

    def disconnect(self):
        with self.lock: connections = list(self.active)
        for conn in connections:
            try:
                if conn.sock:
                    conn.sock.shutdown(socket.SHUT_RDWR)
                conn.close()
            except OSError: pass

    def error(self, handler, status, message):
        raw = encoded({"error": {"message": message, "type": "evaluation_limit", "code": "evaluation_limit"}})
        try:
            handler.send_response(status)
            handler.send_header("Content-Type", "application/json")
            handler.send_header("Content-Length", str(len(raw)))
            handler.end_headers(); handler.wfile.write(raw)
        except OSError: pass

    def admit(self, body: dict, size: int) -> dict:
        self.control.check()
        with self.lock:
            if self.blocked: raise HarnessError("budget", self.blocked)
            if len(self.calls) >= self.limits["max_model_calls"]:
                self.blocked = "Model call budget exhausted"
                raise HarnessError("budget", self.blocked)
            # Fail closed once an earlier completed response omitted usage: cannot assert a token cap.
            if any(c["finished"] and c["tokens"] is None for c in self.calls):
                self.blocked = "Provider omitted usage; remaining token budget is unknown"
                raise HarnessError("budget", self.blocked)
            used = sum(c["tokens"] or 0 for c in self.calls)
            if used >= self.limits["max_tokens"]:
                self.blocked = "Reported token budget exhausted"
                raise HarnessError("budget", self.blocked)
            record = {"ordinal": len(self.calls) + 1, "request_bytes": size, "request_hash": digest(body),
                      "http_status": None, "tokens": None, "finished": False, "duration_ms": None,
                      "first_content_ms": None}
            self.calls.append(record)
        self.emit("model", f"Model attempt {record['ordinal']} admitted")
        return record

    @staticmethod
    def usage_event(value: dict, state: dict) -> bool:
        kind = value.get("type", "")
        container = value.get("response", value.get("message", value))
        usage = container.get("usage") if isinstance(container, dict) else None
        if not isinstance(usage, dict): usage = value.get("usage")
        if isinstance(usage, dict):
            for source, dest in (("input_tokens", "input"), ("prompt_tokens", "input"), ("output_tokens", "output"),
                                 ("completion_tokens", "output"), ("total_tokens", "total"),
                                 ("cache_creation_input_tokens", "cache_create"), ("cache_read_input_tokens", "cache_read")):
                number = usage.get(source)
                if type(number) is int and number >= 0: state[dest] = number
        if kind in ("response.output_text.delta", "response.function_call_arguments.delta"):
            return bool(value.get("delta"))
        if kind == "content_block_delta": return bool(value.get("delta"))
        return any(bool(c.get("delta")) for c in value.get("choices", []) if isinstance(c, dict))

    def fixture(self, body: dict) -> list[dict | str]:
        from adapters.selftest import respond
        memory = {}
        messages = body.get("messages", [])
        prompt = ""
        for msg in messages:
            if msg.get("role") == "user" and isinstance(msg.get("content"), str):
                candidate = msg["content"]
                if not candidate.startswith("[Host context source:"):
                    prompt = candidate
                    respond(candidate, memory)
        answer, call = respond(prompt, memory)
        if answer == "WAIT":
            while True: self.control.sleep(0.05)
        if messages and messages[-1].get("role") == "tool":
            result = messages[-1].get("content", "")
            try:
                envelope = json.loads(result)
                result = envelope.get("content", result)
            except (ValueError, AttributeError): pass
            answer = result if isinstance(result, str) else json.dumps(result, ensure_ascii=False)
            call = None
        if call:
            name, arguments = call
            offered = {t.get("function", {}).get("name") for t in body.get("tools", [])}
            if name not in offered: raise HarnessError("fixture", "Expected fixture tool was not offered")
            delta = {"tool_calls": [{"index": 0, "id": "call-" + uuid.uuid4().hex, "type": "function",
                       "function": {"name": name, "arguments": json.dumps(arguments)}}]}
            finish = "tool_calls"
        else:
            delta = {"content": answer}; finish = "stop"
        return [{"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]},
                {"choices": [], "usage": {"prompt_tokens": 32, "completion_tokens": 16, "total_tokens": 48}}, "[DONE]"]

    def handle(self, handler):
        record = None; conn = None; started = time.monotonic(); usage = {}; sent = False; completed = False
        try:
            auth = handler.headers.get("Authorization", "").removeprefix("Bearer ")
            alternate = handler.headers.get("x-api-key", "")
            if not (hmac.compare_digest(auth, self.token) or hmac.compare_digest(alternate, self.token)):
                self.error(handler, 401, "Invalid evaluator transport token"); return
            if handler.path != self.path:
                self.error(handler, 404, "Unexpected model route"); return
            length = int(handler.headers.get("Content-Length", "0"))
            if not 0 < length <= 4 * 1024 * 1024 or handler.headers.get("Transfer-Encoding"):
                self.error(handler, 413, "Model request exceeds 4 MiB"); return
            handler.connection.settimeout(self.control.remaining(10))
            raw = handler.rfile.read(length)
            if len(raw) != length: raise HarnessError("transport", "Incomplete model request")
            body = json.loads(raw)
            if body.get("model") != self.model["model"]:
                raise HarnessError("configuration", "Model request differs from selected profile")
            record = self.admit(body, len(raw))
            if self.model["kind"] == "fixture":
                values = self.fixture(body)
                handler.send_response(200); handler.send_header("Content-Type", "text/event-stream"); handler.end_headers(); sent = True
                record["http_status"] = 200
                for value in values:
                    self.control.check()
                    if isinstance(value, dict) and self.usage_event(value, usage) and record["first_content_ms"] is None:
                        record["first_content_ms"] = round((time.monotonic() - started) * 1000, 2)
                    handler.wfile.write(b"data: " + (encoded(value) if isinstance(value, dict) else value.encode()) + b"\n\n")
                    handler.wfile.flush()
                completed = True
                return
            target = urlsplit(self.model["endpoint"])
            cls = http.client.HTTPSConnection if target.scheme == "https" else http.client.HTTPConnection
            conn = cls(target.hostname, target.port, timeout=self.control.remaining(30))
            with self.lock: self.active.add(conn)
            headers = {"Content-Type": "application/json", "Accept": handler.headers.get("Accept", "text/event-stream")}
            if self.model["protocol"] == "messages":
                headers.update({"x-api-key": self.model["key"], "anthropic-version": handler.headers.get("anthropic-version", "2023-06-01")})
                if handler.headers.get("anthropic-beta"): headers["anthropic-beta"] = handler.headers["anthropic-beta"]
            else: headers["Authorization"] = "Bearer " + self.model["key"]
            conn.request("POST", target.path or "/", body=raw, headers=headers)
            response = conn.getresponse(); record["http_status"] = response.status
            handler.send_response(response.status)
            handler.send_header("Content-Type", response.getheader("Content-Type", "application/json"))
            for key in ("retry-after", "retry-after-ms"):
                if response.getheader(key): handler.send_header(key, response.getheader(key))
            handler.end_headers(); sent = True
            pending = b""; full = bytearray(); total = 0
            sse = "text/event-stream" in response.getheader("Content-Type", "")
            while True:
                self.control.check()
                chunk = response.read1(65536)
                if not chunk: break
                total += len(chunk)
                if total > 8 * 1024 * 1024: raise HarnessError("capacity", "Model response exceeds observer limit")
                handler.wfile.write(chunk); handler.wfile.flush()
                if sse:
                    pending += chunk
                    if len(pending) > 512 * 1024: raise HarnessError("capacity", "Oversized model event")
                    while b"\n" in pending:
                        line, pending = pending.split(b"\n", 1)
                        if not line.startswith(b"data:"): continue
                        try: value = json.loads(line[5:].strip())
                        except ValueError: continue
                        if isinstance(value, dict) and self.usage_event(value, usage) and record["first_content_ms"] is None:
                            record["first_content_ms"] = round((time.monotonic() - started) * 1000, 2)
                else: full.extend(chunk)
            if not sse:
                try: self.usage_event(json.loads(full), usage)
                except (ValueError, AttributeError): pass
            completed = response.status < 400
        except (HarnessError, ValueError, OSError, http.client.HTTPException) as exc:
            if record: record["transport_error"] = type(exc).__name__
            if not sent: self.error(handler, 429 if isinstance(exc, HarnessError) and exc.code == "budget" else 502, str(exc) if isinstance(exc, HarnessError) else "Model transport failed")
        finally:
            if conn:
                with self.lock: self.active.discard(conn)
                conn.close()
            if record is not None:
                tokens = usage.get("total")
                if tokens is None and "input" in usage and "output" in usage:
                    tokens = usage["input"] + usage["output"] + usage.get("cache_create", 0) + usage.get("cache_read", 0)
                with self.lock:
                    record.update(tokens=tokens, finished=True, completed=completed,
                                  duration_ms=round((time.monotonic() - started) * 1000, 2))
                    if sum(c["tokens"] or 0 for c in self.calls) > self.limits["max_tokens"]:
                        self.blocked = "Reported token budget exceeded by an in-flight response"
                self.emit("model", f"Model attempt {record['ordinal']} finished (HTTP {record['http_status']})")

    def metrics(self):
        with self.lock:
            calls = [dict(c) for c in self.calls]
            known = sum(c["tokens"] or 0 for c in calls)
            complete = all(c["finished"] and c["tokens"] is not None for c in calls)
            return {"model_calls": len(calls), "tokens": known if complete else None, "reported_tokens_lower_bound": known,
                    "usage_complete": complete, "calls": calls, "budget_error": self.blocked,
                    "model_metric_source": "fixture" if self.model["kind"] == "fixture" else "transport_observed",
                    "first_content_ms": next((c["first_content_ms"] for c in calls if c["first_content_ms"] is not None), None),
                    "cost": None}

    def close(self):
        self.closed.set(); self.disconnect()
        self.server.shutdown(); self.server.server_close()
        self.thread.join(timeout=2); self.watcher.join(timeout=2)
