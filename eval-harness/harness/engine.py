from __future__ import annotations

from collections import Counter
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict
import hashlib
import os
from pathlib import Path
import platform
import shutil
import statistics
import threading
import time
import uuid

from adapters.base import Context
from adapters.command import CommandAdapter
from adapters.mona import MonaAdapter
from adapters.selftest import SelftestAdapter
from graders.rules import grade, verdict
from . import __version__
from .catalog import Catalog
from .common import Control, HarnessError, Redactor, digest, encoded, identifier, integer, safe_path, strict, text
from .gateway import ModelGateway
from .storage import Store

TERMINAL = {"passed", "failed", "error", "timeout", "cancelled", "interrupted", "inconclusive", "skipped"}


def footprint(root: Path) -> dict:
    size = 0; files = 0; complete = True; directories = 0
    def on_error(_):
        nonlocal complete
        complete = False
    for directory, dirs, names in os.walk(root, followlinks=False, onerror=on_error):
        directories += 1
        if directories > 10000:
            return {"bytes": size, "files": files, "complete": False}
        kept = []
        for name in dirs:
            path = Path(directory) / name
            try:
                if path.is_symlink() or getattr(path.lstat(), "st_file_attributes", 0) & 0x400:
                    complete = False
                else:
                    kept.append(name)
            except OSError:
                complete = False
        dirs[:] = kept
        for name in names:
            path = Path(directory) / name
            if path.is_symlink(): complete = False; continue
            files += 1
            if files > 10000: return {"bytes": size, "files": files, "complete": False}
            try: size += path.stat().st_size
            except OSError: complete = False
    return {"bytes": size, "files": files, "complete": complete}


def source_hash(root: Path) -> str:
    hash_ = hashlib.sha256()
    for directory in ("harness", "adapters", "graders", "tasks"):
        for path in sorted((root / directory).rglob("*")):
            if path.is_file() and path.suffix in (".py", ".json"):
                hash_.update(path.relative_to(root).as_posix().encode()); hash_.update(path.read_bytes())
    return hash_.hexdigest()


def summary(trials: list[dict]) -> dict:
    counts = dict(Counter(t["status"] for t in trials))
    durations = [t["metrics"]["wall_ms"] for t in trials if "wall_ms" in t.get("metrics", {})]
    calls = [t.get("metrics", {}) for t in trials if t["status"] != "skipped"]
    known = [m for m in calls if m.get("usage_complete") is True]
    ordered = sorted(durations)
    return {"total": len(trials), "counts": counts, "finished": sum(t["status"] in TERMINAL for t in trials),
            "pass_rate": counts.get("passed", 0) / len(trials) if trials else None,
            "median_ms": statistics.median(durations) if durations else None,
            "p95_ms": ordered[min(len(ordered) - 1, max(0, int(len(ordered) * .95 + .999) - 1))] if ordered else None,
            "model_calls": sum(m.get("model_calls", 0) for m in calls),
            "tokens": sum(m.get("tokens", 0) for m in known) if len(known) == len(calls) else None,
            "usage_coverage": len(known) / len(calls) if calls else None,
            "cost": None}


class Manager:
    def __init__(self, root: Path, data: Path, config: Path | None = None):
        self.root = root.resolve(); self.catalog = Catalog(self.root, config)
        self.store = Store(data)
        self.lock = threading.RLock()
        self.active: dict[str, tuple[threading.Event, threading.Thread]] = {}
        self.closed = False

    def start(self, request: dict) -> dict:
        strict(request, {"request_id", "adapter", "model", "suite", "task_ids", "repeats", "concurrency", "timeout_s", "max_model_calls", "max_tokens", "allow_paid", "allow_local_execution", "label"}, {"adapter", "model", "suite"})
        config = {"adapter": identifier(request["adapter"]), "model": identifier(request["model"]),
                  "suite": identifier(request["suite"]), "repeats": integer(request.get("repeats", 1), "repeats", 1, 20),
                  "concurrency": integer(request.get("concurrency", 1), "concurrency", 1, 4),
                  "timeout_s": integer(request.get("timeout_s", 120), "timeout_s", 1, 1800),
                  "max_model_calls": integer(request.get("max_model_calls", 16), "max_model_calls", 1, 200),
                  "max_tokens": integer(request.get("max_tokens", 32000), "max_tokens", 1, 2000000),
                  "label": text(request.get("label", ""), "label", 200, empty=True)}
        request_id = identifier(request.get("request_id", uuid.uuid4().hex), "request_id")
        for field in ("allow_paid", "allow_local_execution"):
            if field in request and type(request[field]) is not bool:
                raise HarnessError("invalid", field + " must be boolean")
        agent = self.catalog.resolve_agent(config["adapter"])
        model = self.catalog.resolve_model(config["model"])
        if agent["kind"] == "selftest" and model["kind"] != "fixture":
            raise HarnessError("invalid", "Self-test adapter cannot produce real-model evaluation scores")
        if model["kind"] == "remote" and request.get("allow_paid") is not True:
            raise HarnessError("authorization", "Explicit authorization required for model calls, even for a self-hosted endpoint")
        if agent["kind"] != "selftest" and request.get("allow_local_execution") is not True:
            raise HarnessError("authorization", "Explicitly allow trusted local Agent execution; this is not a sandbox")
        tasks = [t for t in self.catalog.tasks.values() if config["suite"] == "all" or t["suite"] == config["suite"]]
        if "task_ids" in request:
            ids = request["task_ids"]
            if not isinstance(ids, list) or not ids or len(ids) > 100 or any(x not in {t["id"] for t in tasks} for x in ids):
                raise HarnessError("invalid", "Unknown or empty selected task set")
            tasks = [t for t in tasks if t["id"] in ids]; config["task_ids"] = sorted(set(ids))
        if not tasks or len(tasks) * config["repeats"] > 100:
            raise HarnessError("capacity", "Select 1..100 trials per evaluation")
        if shutil.disk_usage(self.store.root).free < 128 * 1024 * 1024:
            raise HarnessError("capacity", "At least 128 MiB free disk space is required")
        frozen = digest(config)
        with self.lock:
            if self.closed: raise HarnessError("closed", "Runner is stopping")
            with self.store._guard:
                row = self.store.db.execute("SELECT payload FROM runs WHERE request_id=?", (request_id,)).fetchone()
            if row:
                import json
                existing = json.loads(row[0])
                if existing["config_hash"] != frozen: raise HarnessError("conflict", "Request id configuration conflict")
                return existing
            if self.active: raise HarnessError("busy", "Another evaluation is active; cancel it or wait")
            trials = []
            for task in tasks:
                for repeat in range(config["repeats"]):
                    trials.append({"id": f"t-{len(trials) + 1}", "task_id": task["id"], "title": task["title"], "repeat": repeat,
                                   "task_hash": task["fingerprint"], "status": "pending", "checks": [], "metrics": {}})
            mode = "selftest" if agent["kind"] == "selftest" else "controlled" if model["kind"] == "fixture" else "live"
            run = {"version": 1, "id": "e-" + uuid.uuid4().hex, "request_id": request_id, "created": time.time(), "finished": None,
                   "status": "queued", "mode": mode, "config": config, "config_hash": frozen,
                   "manifest": {t["id"]: t for t in tasks}, "taskset_hash": digest([t["fingerprint"] for t in tasks]),
                   "provenance": {"harness_version": __version__, "harness_hash": source_hash(self.root), "grader_hash": hashlib.sha256((self.root / "graders" / "rules.py").read_bytes()).hexdigest(), "python": platform.python_version(),
                                  "platform": platform.platform(), "adapter": agent["kind"], "agent_revision": agent.get("revision"),
                                  "model": model["model"], "protocol": model["protocol"],
                                  "endpoint_hash": digest(model.get("endpoint", "fixture")), "isolation": "local_process"},
                   "warnings": ["Local process isolation is not a security sandbox.", "Token limits stop later calls; a single in-flight response may exceed the remaining reported budget.",
                                "Missing usage and process-memory/disk-I/O measurements are unknown, not zero.", "No external benchmark certification or inferred safety score."],
                   "trials": trials, "summary": summary(trials)}
            if mode != "live": run["warnings"].append("Controlled fixture/self-test: not a model capability evaluation.")
            if agent.get("binary"):
                hash_ = hashlib.sha256()
                with open(agent["binary"], "rb") as handle:
                    for block in iter(lambda: handle.read(1024 * 1024), b""): hash_.update(block)
                run["provenance"]["agent_binary_sha256"] = hash_.hexdigest()
            self.store.create(run)
            cancel = threading.Event()
            worker = threading.Thread(target=self._execute, args=(run, agent, model, cancel), daemon=True)
            self.active[run["id"]] = (cancel, worker)
            worker.start()
            return self.store.get(run["id"])

    def _execute(self, run: dict, agent: dict, model: dict, cancel: threading.Event):
        try:
            with self.lock:
                run["status"] = "cancelling" if cancel.is_set() else "running"
                self.store.save(run)
            with ThreadPoolExecutor(max_workers=run["config"]["concurrency"], thread_name_prefix="eval-trial") as pool:
                futures = {pool.submit(self._trial, run, trial, run["manifest"][trial["task_id"]], run["config"], agent, model, cancel): trial for trial in run["trials"]}
                for future in as_completed(futures):
                    trial = futures[future]
                    try: result = future.result()
                    except Exception:
                        result = dict(trial, status="error", error="Internal evaluation failure; no score was inferred")
                    with self.lock:
                        trial.update(result)
                        run["summary"] = summary(run["trials"])
                        if cancel.is_set(): run["status"] = "cancelling"
                        try:
                            self.store.save(run)
                        except Exception:
                            # Signal peers before ThreadPoolExecutor waits for their exit.
                            cancel.set()
                            raise
            with self.lock:
                run["status"] = "cancelled" if cancel.is_set() else "completed" if all(t["status"] == "passed" for t in run["trials"]) else "completed_with_issues"
                run["finished"] = time.time(); run["summary"] = summary(run["trials"])
                self.store.save(run)
        except Exception:
            cancel.set()
            with self.lock:
                run.update(status="interrupted", finished=time.time(), message="Evaluation persistence failed; result is not a completed measurement")
                for trial in run["trials"]:
                    if trial["status"] not in TERMINAL:
                        trial.update(status="interrupted", error="Coordinator failed before durable completion")
                run["summary"] = summary(run["trials"])
                try: self.store.save(run)
                except Exception: pass
        finally:
            with self.lock: self.active.pop(run["id"], None)

    def _trial(self, run, trial, task, limits, agent, model, cancel):
        rid = run["id"]
        result = dict(trial, metrics={})
        required = set(task.get("requires", []))
        if any(t.get("restart_before") for t in task["turns"]): required.add("restart")
        missing = required - set(agent.get("capabilities", ["text"]))
        if missing:
            return dict(result, status="skipped", error="Adapter missing capabilities: " + ", ".join(sorted(missing)))
        started = time.monotonic()
        control = Control(cancel, started + limits["timeout_s"])
        base = self.store.root / "runs" / rid / trial["id"]
        workspace, state, home = base / "workspace", base / "state", base / "home"
        redactor = Redactor([model.get("key", ""), *agent.get("environment", {}).values()])
        def emit(kind, message):
            self.store.event(rid, {"time": time.time(), "trial": trial["id"], "type": kind, "message": redactor.clean(message[:4096])})
        adapter = gateway = None; observations = []
        result.update(status="running", started=time.time())
        with self.lock:
            trial.update(status="running", started=result["started"])
            run["summary"] = summary(run["trials"])
            self.store.save(run)
        try:
            control.check()
            for path in (workspace, state, home): path.mkdir(parents=True, exist_ok=False)
            for path, content in task.get("fixtures", {}).items():
                target = safe_path(workspace, path); target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8", newline="")
            emit("trial_started", task["title"])
            gateway = ModelGateway(model, limits, control, emit)
            redactor.values.append(gateway.token)
            context = Context(workspace, state, home, control, emit, gateway, agent, model)
            adapter = {"selftest": SelftestAdapter, "command": CommandAdapter, "mona": MonaAdapter}[agent["kind"]](context)
            if getattr(adapter, "token", None): redactor.values.append(adapter.token)
            adapter.start()
            result["metrics"]["setup_ms"] = round((time.monotonic() - started) * 1000, 2)
            execution = time.monotonic()
            for index, turn in enumerate(task["turns"]):
                control.check()
                if turn.get("restart_before"):
                    emit("restart", "Restarting only this trial's Agent host")
                    adapter.restart()
                observation = adapter.turn(turn["prompt"].replace("{workspace}", workspace.as_posix()))
                observations.append(observation)
                if observation.status != "completed": raise HarnessError("agent_failed", "Agent reported task failure")
                metrics = gateway.metrics()
                if metrics["budget_error"]: raise HarnessError("budget", metrics["budget_error"])
                emit("turn_completed", f"Turn {index + 1} completed")
            result["metrics"]["execute_ms"] = round((time.monotonic() - execution) * 1000, 2)
            control.check()
            check_start = time.monotonic()
            result["checks"] = grade(task, observations, workspace)
            result["status"] = verdict(result["checks"])
            if agent["kind"] == "command" and model["kind"] == "remote" and gateway.metrics()["model_calls"] == 0:
                result.update(status="inconclusive", error="No model traffic observed. The adapter may not be using the configured model gateway.")
            result["metrics"]["grading_ms"] = round((time.monotonic() - check_start) * 1000, 2)
        except HarnessError as exc:
            result.update(status=exc.code if exc.code in ("cancelled", "timeout") else "error" if exc.code in ("configuration", "adapter", "transport") else "failed",
                          error=redactor.clean(str(exc))[:4096], error_code=exc.code)
        except Exception as exc:
            result.update(status="error", error=redactor.clean(f"{type(exc).__name__}: {exc}")[:4096])
        finally:
            cleanup_start = time.monotonic()
            for owner in (adapter, gateway):
                if owner:
                    try: owner.close()
                    except Exception:
                        result.update(status="error", error="Owned-resource cleanup failed; inspect isolated execution environment")
            result.setdefault("metrics", {}).update({"wall_ms": round((time.monotonic() - started) * 1000, 2),
                "cleanup_ms": round((time.monotonic() - cleanup_start) * 1000, 2),
                "workspace": footprint(workspace), "state": footprint(state), "peak_rss_bytes": None, "disk_io_bytes": None})
            if gateway: result["metrics"].update(gateway.metrics())
            result["finished"] = time.time()
            result["turns"] = [{"status": o.status, "output": redactor.clean(o.output[:2048]), "output_truncated": len(o.output) > 2048,
                                "tool_events_complete": o.tool_events_complete, "tool_count": len(o.tools) if o.tool_events_complete else None} for o in observations]
            if base.exists():
                evidence = redactor.clean({"version": 1, "task_hash": task["fingerprint"], "observations": [asdict(o) for o in observations],
                                           "checks": result["checks"], "metrics": result["metrics"]})
                raw = encoded(evidence)
                if len(raw) <= 8 * 1024 * 1024:
                    evidence_path = base / "evidence.json"
                    with evidence_path.open("wb") as handle:
                        handle.write(raw); handle.flush(); os.fsync(handle.fileno())
                    result["evidence_sha256"] = hashlib.sha256(raw).hexdigest()
                else: result.update(status="error", error="Evidence exceeded 8 MiB; no success claim retained")
            emit("trial_finished", task["title"] + ": " + result["status"])
        return result

    def cancel(self, rid: str) -> dict:
        with self.lock:
            run = self.store.get(rid)
            if rid in self.active:
                self.active[rid][0].set()
                run["status"] = "cancelling"; self.store.save(run)
            return {"id": rid, "signalled": rid in self.active}

    def delete(self, rid: str):
        identifier(rid)
        with self.lock:
            if rid in self.active: raise HarnessError("conflict", "Cannot remove an active run")
            # Retain the report if file removal fails so the operator can retry.
            self.store.get(rid)
            path = self.store.root / "runs" / rid
            if path.exists(): shutil.rmtree(path)
            self.store.delete(rid)

    def evidence(self, rid: str, tid: str) -> bytes:
        identifier(rid); identifier(tid)
        run = self.store.get(rid)
        trial = next((t for t in run["trials"] if t["id"] == tid), None)
        if not trial or not trial.get("evidence_sha256"):
            raise HarnessError("not_found", "No complete trial evidence available")
        path = safe_path(self.store.root, f"runs/{rid}/{tid}/evidence.json")
        with path.open("rb") as file: data = file.read(8 * 1024 * 1024 + 1)
        if len(data) > 8 * 1024 * 1024 or hashlib.sha256(data).hexdigest() != trial["evidence_sha256"]:
            raise HarnessError("integrity", "Evidence file no longer matches the stored report")
        return data

    def compare(self, left: str, right: str) -> dict:
        a, b = self.store.get(left), self.store.get(right)
        warnings = []
        if a["taskset_hash"] != b["taskset_hash"]: warnings.append("任务集或评分规则不同，不能直接比较总成功率")
        if a["mode"] != b["mode"]: warnings.append("运行性质不同：自检、受控测试和真实模型成绩不能混用")
        if a["provenance"].get("grader_hash") != b["provenance"].get("grader_hash"):
            warnings.append("评分器实现不同")
        for field in ("platform", "python"):
            if a["provenance"].get(field) != b["provenance"].get(field): warnings.append(field + " 环境不同")
        for key in ("timeout_s", "max_model_calls", "max_tokens", "concurrency", "repeats"):
            if a["config"][key] != b["config"][key]: warnings.append(key + " 不同")
        if a["status"] in ("running", "queued", "cancelling") or b["status"] in ("running", "queued", "cancelling"):
            warnings.append("存在未完成运行")
        fields = ("pass_rate", "median_ms", "p95_ms", "model_calls", "tokens")
        metrics = [{"name": k, "left": a["summary"].get(k), "right": b["summary"].get(k),
                    "delta": b["summary"][k] - a["summary"][k] if a["summary"].get(k) is not None and b["summary"].get(k) is not None else None} for k in fields]
        before = {(t["task_id"], t["repeat"], t["task_hash"]): t for t in a["trials"]}
        changes = []
        for t in b["trials"]:
            old = before.get((t["task_id"], t["repeat"], t["task_hash"]))
            if old: changes.append({"task": t["task_id"], "repeat": t["repeat"], "before": old["status"], "after": t["status"]})
        return {"left": left, "right": right, "comparable": not warnings, "warnings": warnings,
                "metrics": metrics, "tasks": changes, "note": "小样本差异不等于统计显著改善；没有自动生成排名或综合安全分。"}

    def close(self):
        with self.lock:
            self.closed = True
            active = list(self.active.values())
            for cancel, _ in active: cancel.set()
        for _, worker in active: worker.join(timeout=45)
        if any(t.is_alive() for _, t in active):
            raise HarnessError("busy", "Owned evaluation workers did not stop within shutdown grace")
        self.store.close()
