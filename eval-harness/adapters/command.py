from __future__ import annotations

import json
import queue

from harness.common import HarnessError, clean_environment, text
from harness.processes import Child
from .base import Adapter, Observation


class CommandAdapter(Adapter):
    """One process per turn, persistent trial state passed explicitly; no shell interpolation."""
    def __init__(self, context):
        super().__init__(context)
        self.child = None
        self.turn_index = 0

    def turn(self, prompt: str) -> Observation:
        c = self.context
        env = clean_environment(c.home)
        env.update(c.agent.get("environment", {}))
        env.update(EVAL_MODEL_ENDPOINT=c.gateway.endpoint, EVAL_MODEL_KEY=c.gateway.token,
                   EVAL_MODEL_NAME=c.model["model"], EVAL_MODEL_PROTOCOL=c.model["protocol"])
        request = {"protocol": 1, "prompt": prompt, "workspace": str(c.workspace), "state_dir": str(c.state),
                   "turn": self.turn_index, "deadline_seconds": c.control.remaining(3600)}
        self.child = Child(c.agent["argv"], c.workspace, env, request)
        final = None; tools = []; count = 0
        try:
            while True:
                c.control.check(); self.child.check()
                try: raw = self.child.lines.get(timeout=0.05)
                except queue.Empty:
                    if self.child.proc.poll() is not None and self.child.stdout_done.is_set(): break
                    continue
                count += 1
                if count > 10000: raise HarnessError("capacity", "Too many adapter events")
                try: event = json.loads(raw)
                except ValueError as exc: raise HarnessError("adapter", "Adapter stdout must be newline-delimited JSON") from exc
                if not isinstance(event, dict): raise HarnessError("adapter", "Invalid adapter event")
                kind = event.get("type")
                if final is not None: raise HarnessError("adapter", "Events after final result are invalid")
                if kind == "text":
                    c.emit("agent_text", text(event.get("text"), "event text", 8192, empty=True))
                elif kind == "tool":
                    for key in ("id", "name", "status"): text(event.get(key), key, 128)
                    if any(t["id"] == event["id"] for t in tools):
                        raise HarnessError("adapter", "Duplicate terminal tool event")
                    tools.append({k: event[k] for k in ("id", "name", "status")})
                    c.emit("tool", f"{event['name']}: {event['status']}")
                elif kind == "result":
                    if event.get("protocol") != 1 or event.get("status") not in ("completed", "failed"):
                        raise HarnessError("adapter", "Invalid protocol or result status")
                    final = Observation(text(event.get("output", ""), "output", empty=True), event["status"], tools,
                                        event.get("tool_events_complete") is True,
                                        {"adapter_metric_source": "reported", "adapter_revision": c.agent.get("revision")})
                else: raise HarnessError("adapter", "Unknown adapter event")
            self.child.check()
            if self.child.proc.returncode != 0:
                raise HarnessError("adapter", "Agent process exited unsuccessfully: " + self.child.error_tail())
            if final is None: raise HarnessError("adapter", "Agent exited without a final result")
            self.turn_index += 1
            return final
        finally:
            self.close()

    def restart(self) -> None:
        # Each turn is already a fresh process. The adapter must recover from state_dir.
        self.close()

    def close(self) -> None:
        if self.child: self.child.close(); self.child = None
