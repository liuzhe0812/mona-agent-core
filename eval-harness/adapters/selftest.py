"""Deterministic harness self-test only. Never represents model intelligence."""
from __future__ import annotations

import json
import re
from pathlib import Path

from harness.common import read_artifact, safe_path, load_json, encoded
from .base import Adapter, Observation


def respond(prompt: str, memory: dict) -> tuple[str, tuple[str, dict] | None]:
    """Small explicit fixture vocabulary. No access to task manifests or grader expectations."""
    if prompt.startswith("Reply with exactly "):
        return prompt.removeprefix("Reply with exactly ").strip(), None
    if prompt.startswith("Return JSON "):
        return json.dumps(json.loads(prompt.removeprefix("Return JSON ")), ensure_ascii=False), None
    match = re.search(r"Remember project code ([A-Z0-9_-]+)", prompt)
    if match:
        memory["project"] = match[1]; return "Recorded for this session.", None
    if prompt.startswith("What project code"):
        return memory.get("project", "UNKNOWN"), None
    if prompt.startswith("Read file "):
        path = prompt.removeprefix("Read file ").splitlines()[0].strip(); return "", ("read", {"path": path})
    if prompt.startswith("Write file "):
        path, content = prompt.removeprefix("Write file ").split("\nContent:\n", 1)
        return "", ("write", {"path": path, "content": content})
    if prompt.startswith("Wait until cancelled"):
        return "WAIT", None
    return "UNSUPPORTED_SELFTEST_PROMPT", None


class SelftestAdapter(Adapter):
    def __init__(self, context):
        super().__init__(context)
        self.path = context.state / "fixture.json"
        self.memory = {}

    def start(self):
        if self.path.exists(): self.memory = load_json(self.path)

    def turn(self, prompt: str) -> Observation:
        c = self.context; c.control.check()
        answer, call = respond(prompt, self.memory)
        self.path.write_bytes(encoded(self.memory))
        tools = []
        if answer == "WAIT":
            while True: c.control.sleep(0.05)
        if call:
            name, args = call
            path = Path(args["path"]).resolve()
            relative = path.relative_to(c.workspace.resolve()).as_posix()
            if name == "read": answer = read_artifact(c.workspace, relative)
            else:
                target = safe_path(c.workspace, relative)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(args["content"], encoding="utf-8")
                answer = "File written."
            tools.append({"id": "fixture-1", "name": name, "status": "success"})
            c.emit("tool", name + ": success")
        c.emit("agent_text", answer[:2048])
        return Observation(answer, tools=tools, tool_events_complete=True, metrics={"fixture": True})

    def restart(self):
        self.memory = {}; self.start()
