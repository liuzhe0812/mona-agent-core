from __future__ import annotations

import copy
import os
from pathlib import Path
import sys
from urllib.parse import urlsplit

from .common import HarnessError, identifier, integer, load_json, strict, text, digest

PROTOCOLS = {"chat_completions", "responses", "messages"}
CHECKS = {"exact", "contains", "json", "file_exact", "file_json", "absent", "unchanged", "tool_count"}


class Catalog:
    def __init__(self, root: Path, config: Path | None = None):
        self.root = root.resolve()
        source = load_json(config) if config else {"version": 1, "agents": [], "models": []}
        strict(source, {"version", "agents", "models"}, {"version"})
        if source["version"] != 1:
            raise HarnessError("invalid", "Only configuration format 1 is supported")
        for field in ("agents", "models"):
            if not isinstance(source.get(field, []), list) or len(source.get(field, [])) > 64:
                raise HarnessError("invalid", "At most 64 agent/model profiles are accepted")
        self.agents = {
            "selftest": {"id": "selftest", "label": "测评引擎自检（非 Agent 能力成绩）", "kind": "selftest", "capabilities": ["text", "files", "multiturn", "restart", "tool_events"]},
            "mona": {"id": "mona", "label": "Mona · 独立测试宿主", "kind": "mona", "binary_env": "MONA_EVAL_SERVER", "capabilities": ["text", "files", "multiturn", "restart", "tool_events"]},
        }
        self.models = {"fixture": {"id": "fixture", "label": "受控本地模型（免费、非能力评测）", "kind": "fixture", "protocol": "chat_completions", "model": "eval-fixture"}}
        for agent in source.get("agents", []):
            strict(agent, {"id", "label", "kind", "argv", "capabilities", "env_keys", "binary_env", "revision"}, {"id", "kind", "label"})
            aid = identifier(agent["id"])
            if aid in self.agents or agent["kind"] not in ("command", "mona"):
                raise HarnessError("invalid", "Duplicate agent id or unsupported adapter kind")
            text(agent["label"], "agent label", 200)
            if agent["kind"] == "command":
                argv = agent.get("argv")
                if not isinstance(argv, list) or not 1 <= len(argv) <= 32:
                    raise HarnessError("invalid", "command adapter needs literal argv, not a shell command")
                for arg in argv: text(arg, "argv", 2048)
            if not isinstance(agent.get("env_keys", []), list) or len(agent.get("env_keys", [])) > 16:
                raise HarnessError("invalid", "env_keys must contain at most 16 variable names")
            reserved = {"HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_STATE_HOME", "TMP", "TEMP", "TMPDIR"}
            for name in agent.get("env_keys", []):
                identifier(name, "environment variable")
                if name.upper() in reserved or name.upper().startswith("EVAL_MODEL_"):
                    raise HarnessError("invalid", "Adapter environment cannot override isolated state or model routing")
            if "binary_env" in agent: identifier(agent["binary_env"], "binary_env")
            if "revision" in agent: text(agent["revision"], "revision", 200)
            caps = agent.get("capabilities", ["text"])
            if not isinstance(caps, list) or len(caps) > 32:
                raise HarnessError("invalid", "Invalid adapter capabilities")
            for cap in caps: identifier(cap, "capability")
            self.agents[aid] = agent
        for model in source.get("models", []):
            strict(model, {"id", "label", "kind", "protocol", "model", "endpoint_env", "key_env", "context_window"}, {"id", "label", "protocol", "model", "endpoint_env", "key_env"})
            mid = identifier(model["id"])
            if mid in self.models or model["protocol"] not in PROTOCOLS:
                raise HarnessError("invalid", "Duplicate model or unsupported protocol")
            for key in ("label", "model"): text(model[key], key, 200)
            for key in ("endpoint_env", "key_env"): identifier(model[key], key)
            if "context_window" in model: integer(model["context_window"], "context_window", 1024, 10**9)
            self.models[mid] = dict(model, kind="remote")
        self.tasks = {}
        for path in sorted((root / "tasks").glob("*.json")):
            task = load_json(path, 1024 * 1024)
            self.validate_task(task)
            if task["id"] in self.tasks:
                raise HarnessError("invalid", "Duplicate task id")
            task["fingerprint"] = digest(task)
            self.tasks[task["id"]] = task
        if not self.tasks:
            raise HarnessError("invalid", "No task manifests installed")

    @staticmethod
    def validate_task(task: dict) -> None:
        strict(task, {"version", "id", "title", "suite", "requires", "fixtures", "turns", "checks", "description"}, {"version", "id", "title", "suite", "turns", "checks"})
        if task["version"] != 1: raise HarnessError("invalid", "Task format must be 1")
        for key in ("id", "suite"): identifier(task[key], key)
        text(task["title"], "title", 200)
        if "description" in task: text(task["description"], "description", 2048)
        if not isinstance(task.get("requires", []), list) or len(task.get("requires", [])) > 32:
            raise HarnessError("invalid", "requires must be a bounded capability list")
        for capability in task.get("requires", []): identifier(capability, "capability")
        if not isinstance(task["turns"], list) or not 1 <= len(task["turns"]) <= 20:
            raise HarnessError("invalid", "Task needs 1..20 turns")
        for turn in task["turns"]:
            strict(turn, {"prompt", "restart_before"}, {"prompt"})
            text(turn["prompt"], "prompt")
            if "restart_before" in turn and type(turn["restart_before"]) is not bool:
                raise HarnessError("invalid", "restart_before must be boolean")
        fixtures = task.get("fixtures", {})
        if not isinstance(fixtures, dict) or len(fixtures) > 32:
            raise HarnessError("invalid", "Too many fixtures")
        from .common import safe_path
        for path, content in fixtures.items():
            safe_path(Path.cwd(), path)
            text(content, "fixture", 262144, empty=True)
        if sum(len(value.encode("utf-8")) for value in fixtures.values()) > 1024 * 1024:
            raise HarnessError("capacity", "Total fixture content is limited to 1 MiB")
        if not isinstance(task["checks"], list) or not 1 <= len(task["checks"]) <= 32:
            raise HarnessError("invalid", "Each task needs 1..32 checks")
        for check in task["checks"]:
            strict(check, {"type", "value", "path", "name", "count", "turn", "required"}, {"type"})
            kind = check["type"]
            if kind not in CHECKS: raise HarnessError("invalid", "Unknown grader")
            if kind.startswith("file_") or kind in ("absent", "unchanged"):
                safe_path(Path.cwd(), check.get("path", ""))
            if kind == "unchanged" and check["path"] not in fixtures:
                raise HarnessError("invalid", "unchanged requires an original fixture")
            if kind in ("exact", "contains", "file_exact"): text(check.get("value"), "expected", 1048576, empty=True)
            if kind in ("json", "file_json") and "value" not in check:
                raise HarnessError("invalid", "JSON grader needs expected value")
            if kind == "tool_count":
                identifier(check.get("name"), "tool name"); integer(check.get("count"), "count", 0, 10000)
            if "turn" in check: integer(check["turn"], "turn", 0, len(task["turns"]) - 1)
            if "required" in check and type(check["required"]) is not bool:
                raise HarnessError("invalid", "required must be boolean")

    def resolve_model(self, mid: str) -> dict:
        if mid not in self.models: raise HarnessError("invalid", "Unknown model profile")
        model = copy.deepcopy(self.models[mid])
        if model["kind"] == "remote":
            endpoint = os.environ.get(model["endpoint_env"], "")
            key = os.environ.get(model["key_env"], "")
            u = urlsplit(endpoint)
            if not endpoint or not key:
                raise HarnessError("configuration", "Set the endpoint/key environment variables for this model profile")
            if u.username or u.password or u.query or u.fragment or not u.hostname:
                raise HarnessError("configuration", "Endpoint must not contain credentials, query or fragment")
            if u.scheme != "https" and not (u.scheme == "http" and u.hostname in ("127.0.0.1", "localhost", "::1")):
                raise HarnessError("configuration", "Use HTTPS, or loopback HTTP for a local model")
            model.update(endpoint=endpoint, key=key)
        return model

    def resolve_agent(self, aid: str) -> dict:
        if aid not in self.agents: raise HarnessError("invalid", "Unknown agent adapter")
        agent = copy.deepcopy(self.agents[aid])
        if agent["kind"] == "mona":
            raw = os.environ.get(agent.get("binary_env", "MONA_EVAL_SERVER"), "")
            if not raw or not Path(raw).is_absolute() or not Path(raw).is_file():
                raise HarnessError("configuration", "Set MONA_EVAL_SERVER to an absolute path to the built server executable")
            agent["binary"] = str(Path(raw).resolve())
        if agent["kind"] == "command":
            agent["argv"] = [sys.executable if a == "{python}" else a for a in agent["argv"]]
            agent["environment"] = {k: os.environ[k] for k in agent.get("env_keys", []) if k in os.environ}
        return agent

    def public(self) -> dict:
        agents, models = [], []
        for aid, spec in self.agents.items():
            try: self.resolve_agent(aid); available, reason = True, ""
            except HarnessError as exc: available, reason = False, str(exc)
            agents.append({"id": aid, "label": spec["label"], "kind": spec["kind"], "available": available, "reason": reason, "capabilities": spec.get("capabilities", ["text"])})
        for mid, spec in self.models.items():
            try: self.resolve_model(mid); available, reason = True, ""
            except HarnessError as exc: available, reason = False, str(exc)
            models.append({"id": mid, "label": spec["label"], "protocol": spec["protocol"], "paid": spec["kind"] == "remote", "available": available, "reason": reason})
        suites = sorted({t["suite"] for t in self.tasks.values()})
        return {"agents": agents, "models": models, "suites": [{"id": s, "count": sum(t["suite"] == s for t in self.tasks.values())} for s in suites],
                "tasks": [{k: t.get(k) for k in ("id", "title", "suite", "requires", "description")} for t in self.tasks.values()],
                "isolation": "local_process", "warning": "独立目录不是安全沙箱。仅运行可信 Agent 和审核过的任务；正式模型测评需显式授权。"}
