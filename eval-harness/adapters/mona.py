from __future__ import annotations

import json
import socket
import time
import urllib.error
import urllib.request
import uuid

from harness.common import HarnessError, clean_environment, encoded, text
from harness.processes import Child
from .base import Adapter, Observation


class MonaAdapter(Adapter):
    """Starts a dedicated built Mona host. Never connects to the user's existing service."""
    def __init__(self, context):
        super().__init__(context)
        self.child = None
        self.session_id = None
        self.run_id = None
        self.token = uuid.uuid4().hex + uuid.uuid4().hex
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, method: str, path: str, body=None, *, cleanup=False):
        timeout = 2 if cleanup else self.context.control.remaining(3)
        req = urllib.request.Request(self.endpoint + path, method=method, data=encoded(body) if body is not None else None,
                                     headers={"Authorization": "Bearer " + self.token, "Content-Type": "application/json"})
        try:
            with self.opener.open(req, timeout=timeout) as response:
                data = response.read(2 * 1024 * 1024 + 1)
                if len(data) > 2 * 1024 * 1024: raise HarnessError("capacity", "Mona response exceeds capture limit")
                return json.loads(data) if data else None
        except urllib.error.HTTPError as exc:
            # Never retry POST automatically: admission may already have committed.
            message = exc.read(2048).decode("utf-8", errors="replace")
            raise HarnessError("adapter", f"Mona {method} {path.split('?')[0]} returned HTTP {exc.code}: {message}") from exc

    def start(self):
        c = self.context
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
        self.endpoint = f"http://127.0.0.1:{port}"
        env = clean_environment(c.home)
        env.update({"AGENT_SERVER_TOKEN": self.token, "AGENT_SERVER_ADDR": f"127.0.0.1:{port}",
                    "MONA_DEV_STATE_DIR": str(c.state), "AGENT_WORKSPACE_DIR": str(c.workspace),
                    "AGENT_SESSIONS_DIR": str(c.state / "sessions"), "AGENT_MEMORY_DIR": str(c.state / "memory"),
                    "AGENT_SPILL_DIR": str(c.state / "spill"), "AGENT_CAPABILITY_STATE_PATH": str(c.state / "capabilities.json"),
                    "AGENT_WORKSPACE_SETTINGS_PATH": str(c.state / "workspace.json"), "AGENT_PROJECTS_PATH": str(c.state / "projects.json"),
                    "AGENT_MODEL_MANAGEMENT": "0", "AGENT_MODEL_PROTOCOL": c.model["protocol"],
                    "AGENT_MODEL_ENDPOINT": c.gateway.endpoint, "AGENT_MODEL_KEY": c.gateway.token,
                    "AGENT_MODEL_NAME": c.model["model"], "AGENT_ALLOW_HTTP_LOOPBACK": "1",
                    "AGENT_PROJECTS": "0", "AGENT_SKILLS": "0", "AGENT_INSTRUCTIONS": "0"})
        if c.model.get("context_window"): env["AGENT_MODEL_CONTEXT_TOKENS"] = str(c.model["context_window"])
        self.child = Child([c.agent["binary"]], c.workspace, env)
        deadline = time.monotonic() + min(20, c.control.remaining(20))
        while True:
            c.control.check(); self.child.check()
            if self.child.proc.poll() is not None:
                raise HarnessError("adapter", "Mona startup failed: " + self.child.error_tail())
            try:
                info = self.request("GET", "/v1/info")
                if info.get("protocol_version") != 2: raise HarnessError("adapter", "Unsupported Mona stream protocol")
                break
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                if time.monotonic() >= deadline:
                    raise HarnessError("adapter", "Mona did not become ready on its dedicated port")
                c.control.sleep(0.1)
        if self.session_id is None:
            header = self.request("POST", "/api/sessions", {"request_id": "eval-" + uuid.uuid4().hex})
            self.session_id = header["id"]
        c.emit("lifecycle", "Mona isolated host ready")

    def turn(self, prompt: str) -> Observation:
        c = self.context
        page = self.request("GET", f"/api/sessions/{self.session_id}")
        header = page.get("header", page.get("session", page))
        started = self.request("POST", f"/api/sessions/{self.session_id}/turns", {
            "request_id": "eval-" + uuid.uuid4().hex, "revision": header["revision"], "prompt": prompt})
        self.run_id = started["run_id"]
        if not self.run_id: raise HarnessError("adapter", "Mona did not provide a live Run")
        observed = {}; texts = {}; complete = True
        while True:
            c.control.check(); self.child.check()
            snapshot = self.request("GET", f"/v1/runs/{self.run_id}/snapshot")
            complete = complete and snapshot.get("pruned_items", 0) == 0
            for item in snapshot.get("items", []):
                content = item["content"]
                if content["kind"] == "agent_message":
                    value = content["text"]
                    if texts.get(item["id"]) != value:
                        c.emit("agent_text", value[-2048:]); texts[item["id"]] = value
                elif content["kind"] == "tool_call" and content.get("result") is not None:
                    key = item["id"]
                    if key not in observed:
                        result = content["result"]
                        observed[key] = {"id": result["call_id"], "name": content["name"], "status": result["status"]}
                        c.emit("tool", content["name"] + ": " + result["status"])
            outcome = snapshot.get("outcome")
            if outcome is not None:
                self.run_id = None
                if outcome["status"] != "completed":
                    raise HarnessError("agent_failed", "Mona Run " + outcome["status"] + ": " + str(outcome.get("error") or ""))
                usage = outcome.get("task_usage", {})
                return Observation(text(outcome.get("output") or "", "Mona output", empty=True), tools=list(observed.values()),
                                   tool_events_complete=complete, metrics={"steps": outcome.get("steps"), "mona_usage": usage})
            c.control.sleep(0.08)

    def restart(self):
        self.close(); self.start()

    def close(self):
        if self.run_id:
            try: self.request("POST", f"/v1/runs/{self.run_id}/cancel", {}, cleanup=True)
            except Exception: pass
            self.run_id = None
        if self.child:
            self.child.close(); self.child = None
