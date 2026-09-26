from __future__ import annotations

import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import threading
import time
from typing import Any


class HarnessError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


def encoded(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")


def digest(value: Any) -> str:
    return hashlib.sha256(encoded(value)).hexdigest()


def identifier(value: Any, name: str = "id") -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,95}", value):
        raise HarnessError("invalid", f"{name} must be a bounded identifier")
    return value


def integer(value: Any, name: str, low: int, high: int) -> int:
    if type(value) is not int or not low <= value <= high:
        raise HarnessError("invalid", f"{name} must be an integer in {low}..{high}")
    return value


def text(value: Any, name: str, maximum: int = 65536, empty: bool = False) -> str:
    if not isinstance(value, str) or (not value and not empty) or len(value.encode("utf-8")) > maximum:
        raise HarnessError("invalid", f"{name} must be bounded UTF-8 text (max {maximum} bytes)")
    return value


def strict(value: Any, allowed: set[str], required: set[str] | None = None) -> dict:
    if not isinstance(value, dict) or value.keys() - allowed or (required or set()) - value.keys():
        raise HarnessError("invalid", "Unexpected or missing object fields")
    return value


def load_json(path: Path, maximum: int = 4 * 1024 * 1024) -> Any:
    with path.open("rb") as file:
        data = file.read(maximum + 1)
    if len(data) > maximum:
        raise HarnessError("capacity", "JSON file exceeds size limit")
    try:
        return json.loads(data, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite")))
    except (ValueError, UnicodeError) as exc:
        raise HarnessError("invalid", "Invalid JSON document") from exc


def safe_path(root: Path, relative: str) -> Path:
    text(relative, "relative path", 512)
    name = PurePosixPath(relative)
    if name.is_absolute() or "\\" in relative or ":" in relative or any(x in ("..", ".", "") for x in relative.split("/")):
        raise HarnessError("invalid", "Path must remain inside the trial workspace")
    if root.is_symlink() or (root.exists() and getattr(root.lstat(), "st_file_attributes", 0) & 0x400):
        raise HarnessError("invalid", "Workspace root must not be a link or junction")
    candidate = root
    for part in name.parts:
        candidate = candidate / part
        if candidate.is_symlink() or (candidate.exists() and getattr(candidate.lstat(), "st_file_attributes", 0) & 0x400):
            raise HarnessError("invalid", "Symlinks and junctions are not accepted as task artifacts")
    if not candidate.resolve().is_relative_to(root.resolve()):
        raise HarnessError("invalid", "Artifact path escapes the workspace")
    return candidate


def read_artifact(root: Path, relative: str, maximum: int = 1024 * 1024) -> str:
    path = safe_path(root, relative)
    if not path.is_file():
        raise HarnessError("artifact", f"Missing file: {relative}")
    with path.open("rb") as handle:
        data = handle.read(maximum + 1)
    if len(data) > maximum:
        raise HarnessError("capacity", "Artifact exceeds grading read limit")
    try:
        return data.decode("utf-8")
    except UnicodeError as exc:
        raise HarnessError("artifact", "Artifact is not UTF-8") from exc


class Redactor:
    def __init__(self, values: list[str] | None = None):
        self.values = sorted({v for v in values or [] if v and len(v) >= 4}, key=len, reverse=True)

    def clean(self, value: Any) -> Any:
        if isinstance(value, str):
            for secret in self.values:
                value = value.replace(secret, "[REDACTED]")
            value = re.sub(r"(?i)(bearer\s+)[^\s\"']+", r"\1[REDACTED]", value)
            return value
        if isinstance(value, dict):
            return {k: "[REDACTED]" if re.search(r"(?i)(api.?key|authorization|password|secret)", k)
                    else self.clean(v) for k, v in value.items()}
        if isinstance(value, list):
            return [self.clean(v) for v in value]
        return value


class Control:
    def __init__(self, cancel: threading.Event, deadline: float):
        self.cancel = cancel
        self.deadline = deadline

    def check(self) -> None:
        if self.cancel.is_set():
            raise HarnessError("cancelled", "Cancelled by operator")
        if time.monotonic() >= self.deadline:
            raise HarnessError("timeout", "Trial deadline exceeded")

    def remaining(self, maximum: float = 5) -> float:
        self.check()
        return max(0.1, min(maximum, self.deadline - time.monotonic()))

    def sleep(self, seconds: float) -> None:
        self.cancel.wait(min(seconds, self.remaining(seconds)))
        self.check()


def clean_environment(home: Path) -> dict[str, str]:
    # Do not inherit Agent credentials, proxy variables, real HOME or model configuration.
    names = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "LANG", "LC_ALL"}
    env = {k: v for k, v in os.environ.items() if k.upper() in names}
    home.mkdir(parents=True, exist_ok=True)
    temp = home / "tmp"
    temp.mkdir(exist_ok=True)
    env.update({"HOME": str(home), "USERPROFILE": str(home), "APPDATA": str(home / "appdata"),
                "LOCALAPPDATA": str(home / "local"), "XDG_STATE_HOME": str(home / "state"),
                "TMP": str(temp), "TEMP": str(temp), "TMPDIR": str(temp), "PYTHONUTF8": "1",
                "PYTHONDONTWRITEBYTECODE": "1", "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost"})
    return env
