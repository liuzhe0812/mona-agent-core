from __future__ import annotations

from collections import deque
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading

from .common import HarnessError, encoded


class Child:
    def __init__(self, argv: list[str], cwd: Path, env: dict[str, str], request: dict | None = None):
        self.lines: queue.Queue[bytes] = queue.Queue(256)
        self.stderr = deque(maxlen=32)
        self.overflow = threading.Event()
        self.stdout_done = threading.Event()
        self._closed = False
        supervisor = Path(__file__).with_name("supervisor.py")
        host_env = {k: v for k, v in os.environ.items() if k.upper() in {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"}}
        host_env.update(PYTHONUTF8="1", PYTHONDONTWRITEBYTECODE="1")
        self.proc = subprocess.Popen([sys.executable, "-u", str(supervisor)], cwd=cwd, env=host_env,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.threads = []
        for stream, err in ((self.proc.stdout, False), (self.proc.stderr, True)):
            thread = threading.Thread(target=self._pump, args=(stream, err), daemon=True)
            thread.start(); self.threads.append(thread)
        try:
            self.proc.stdin.write(encoded({"argv": argv, "cwd": str(cwd), "env": env, "input": request}) + b"\n")
            self.proc.stdin.flush()
        except Exception:
            self.close(); raise

    def _pump(self, stream, err: bool) -> None:
        size = 0
        try:
            while True:
                raw = stream.readline(65537)
                if not raw: break
                size += len(raw)
                if len(raw) > 65536 or size > 2 * 1024 * 1024:
                    self.overflow.set(); break
                if err:
                    self.stderr.append(raw.decode("utf-8", errors="replace")[:2048])
                else:
                    try: self.lines.put(raw, timeout=0.2)
                    except queue.Full:
                        self.overflow.set(); break
        finally:
            if not err: self.stdout_done.set()
            stream.close()

    def check(self) -> None:
        if self.overflow.is_set():
            raise HarnessError("capacity", "Child output exceeded bounded capture capacity")

    def error_tail(self) -> str:
        return "".join(self.stderr)[-4096:]

    def close(self) -> None:
        if self._closed: return
        self._closed = True
        try: self.proc.stdin.close()
        except (OSError, ValueError): pass
        try: self.proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            self.proc.kill(); self.proc.wait(timeout=5)
        for thread in self.threads: thread.join(timeout=2)
