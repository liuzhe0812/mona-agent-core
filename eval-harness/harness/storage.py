from __future__ import annotations

from contextlib import contextmanager
from pathlib import Path
import os
import sqlite3
import threading
import time
import json

from .common import HarnessError, encoded


class Store:
    """One local owner, durable reports, bounded log tails. Not a target Agent datastore."""
    def __init__(self, root: Path):
        self.root = root.resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        self._guard = threading.RLock()
        self._lease = (self.root / "owner.lock").open("a+b")
        try:
            if os.fstat(self._lease.fileno()).st_size == 0:
                self._lease.write(b"0")
                self._lease.flush()
            self._lease.seek(0)
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(self._lease.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(self._lease.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as exc:
            self._lease.close()
            raise HarnessError("busy", "This data directory is already in use; close the other runner") from exc
        try:
            self.db = sqlite3.connect(self.root / "reports.sqlite3", check_same_thread=False, timeout=10)
            self.db.execute("PRAGMA journal_mode=WAL")
            self.db.execute("PRAGMA synchronous=FULL")
            self.db.executescript("""
                CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY, created REAL NOT NULL, request_id TEXT UNIQUE, payload TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT, run TEXT NOT NULL, payload TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS event_run ON events(run,seq);
            """)
            for rid, payload in self.db.execute("SELECT id,payload FROM runs").fetchall():
                run = json.loads(payload)
                if run["status"] in ("queued", "running", "cancelling"):
                    run.update(status="interrupted", finished=time.time(), message="Runner stopped; no task was automatically replayed")
                    for trial in run["trials"]:
                        if trial["status"] in ("pending", "running"):
                            trial.update(status="interrupted", error="Runner stopped before durable completion")
                    from .engine import summary
                    run["summary"] = summary(run["trials"])
                    self.db.execute("UPDATE runs SET payload=? WHERE id=?", (encoded(run).decode(), rid))
            self.db.commit()
        except Exception:
            if hasattr(self, "db"):
                self.db.close()
            self._lease.close()
            raise

    def create(self, run: dict) -> dict:
        with self._guard:
            previous = self.db.execute("SELECT payload FROM runs WHERE request_id=?", (run["request_id"],)).fetchone()
            if previous:
                saved = json.loads(previous[0])
                if saved["config_hash"] != run["config_hash"]:
                    raise HarnessError("conflict", "Request id was already used with another configuration")
                return saved
            if self.db.execute("SELECT COUNT(*) FROM runs").fetchone()[0] >= 1000:
                raise HarnessError("capacity", "Report capacity (1000 runs) reached; export and remove completed runs")
            with self.db:
                self.db.execute("INSERT INTO runs VALUES (?,?,?,?)", (run["id"], run["created"], run["request_id"], encoded(run).decode()))
            return run

    def save(self, run: dict) -> None:
        raw = encoded(run)
        if len(raw) > 16 * 1024 * 1024:
            raise HarnessError("capacity", "Report exceeds 16 MiB; evaluation stopped")
        with self._guard, self.db:
            self.db.execute("UPDATE runs SET payload=? WHERE id=?", (raw.decode(), run["id"]))

    def get(self, rid: str) -> dict:
        with self._guard:
            row = self.db.execute("SELECT payload FROM runs WHERE id=?", (rid,)).fetchone()
        if not row:
            raise HarnessError("not_found", "Run not found")
        return json.loads(row[0])

    def list(self, offset: int = 0, limit: int = 50) -> dict:
        with self._guard:
            rows = self.db.execute("SELECT payload FROM runs ORDER BY created DESC LIMIT ? OFFSET ?", (limit, offset)).fetchall()
            total = self.db.execute("SELECT COUNT(*) FROM runs").fetchone()[0]
        runs = []
        for row in rows:
            run = json.loads(row[0])
            runs.append({k: v for k, v in run.items() if k not in ("trials", "manifest")})
        return {"runs": runs, "total": total, "offset": offset}

    def event(self, rid: str, event: dict) -> None:
        raw = encoded(event)
        if len(raw) > 8192:
            event = {"time": time.time(), "type": "log_truncated", "message": "Oversized display event omitted"}
        with self._guard, self.db:
            self.db.execute("INSERT INTO events(run,payload) VALUES (?,?)", (rid, encoded(event).decode()))
            self.db.execute("DELETE FROM events WHERE run=? AND seq NOT IN (SELECT seq FROM events WHERE run=? ORDER BY seq DESC LIMIT 2000)", (rid, rid))

    def events(self, rid: str, after: int = 0) -> dict:
        self.get(rid)
        with self._guard:
            bounds = self.db.execute("SELECT MIN(seq),MAX(seq) FROM events WHERE run=?", (rid,)).fetchone()
            rows = self.db.execute("SELECT seq,payload FROM events WHERE run=? AND seq>? ORDER BY seq LIMIT 200", (rid, after)).fetchall()
        return {"events": [dict(json.loads(p), seq=s) for s, p in rows],
                "next": rows[-1][0] if rows else after, "latest": bounds[1] or 0,
                "truncated": bool(after and bounds[0] and after < bounds[0] - 1)}

    def delete(self, rid: str) -> None:
        run = self.get(rid)
        if run["status"] in ("queued", "running", "cancelling"):
            raise HarnessError("conflict", "Stop the run before removing its report")
        with self._guard, self.db:
            self.db.execute("DELETE FROM events WHERE run=?", (rid,))
            self.db.execute("DELETE FROM runs WHERE id=?", (rid,))

    def close(self) -> None:
        with self._guard:
            self.db.close()
            self._lease.close()
