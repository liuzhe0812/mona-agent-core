"""Private child owner. EOF on the harness pipe tears down this child's process tree.

Launched by processes.py, not a general-purpose shell endpoint. The executable receives
only its declared environment; configuration is passed through a pipe, never argv files.
"""
from __future__ import annotations

import ctypes
import json
import os
import signal
import subprocess
import sys
import threading
import time


def windows_job(process):
    from ctypes import wintypes as w
    class IO(ctypes.Structure):
        _fields_ = [(n, ctypes.c_ulonglong) for n in ("ReadOperationCount", "WriteOperationCount", "OtherOperationCount", "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]
    class Basic(ctypes.Structure):
        _fields_ = [("PerProcessUserTimeLimit", ctypes.c_longlong), ("PerJobUserTimeLimit", ctypes.c_longlong),
                    ("LimitFlags", w.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t), ("MaximumWorkingSetSize", ctypes.c_size_t),
                    ("ActiveProcessLimit", w.DWORD), ("Affinity", ctypes.c_size_t), ("PriorityClass", w.DWORD), ("SchedulingClass", w.DWORD)]
    class Extended(ctypes.Structure):
        _fields_ = [("BasicLimitInformation", Basic), ("IoInfo", IO), ("ProcessMemoryLimit", ctypes.c_size_t),
                    ("JobMemoryLimit", ctypes.c_size_t), ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, w.LPCWSTR]; kernel.CreateJobObjectW.restype = w.HANDLE
    kernel.SetInformationJobObject.argtypes = [w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD]
    kernel.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]
    kernel.CloseHandle.argtypes = [w.HANDLE]
    job = kernel.CreateJobObjectW(None, None)
    info = Extended(); info.BasicLimitInformation.LimitFlags = 0x2000  # KILL_ON_JOB_CLOSE
    if not job or not kernel.SetInformationJobObject(job, 9, ctypes.byref(info), ctypes.sizeof(info)) or not kernel.AssignProcessToJobObject(job, w.HANDLE(int(process._handle))):
        if job: kernel.CloseHandle(job)
        process.kill(); process.wait()
        raise RuntimeError("Cannot establish owned Windows job; execution refused")
    return lambda: kernel.CloseHandle(job)


def main() -> int:
    line = sys.stdin.buffer.readline(131073)
    if not line.endswith(b"\n") or len(line) > 131072:
        return 121
    spec = json.loads(line)
    child = subprocess.Popen(spec["argv"], cwd=spec["cwd"], env=spec["env"], stdin=subprocess.PIPE,
                             stdout=sys.stdout.buffer, stderr=sys.stderr.buffer,
                             start_new_session=os.name != "nt", close_fds=True)
    close_job = windows_job(child) if os.name == "nt" else None
    lost = threading.Event()
    def watch_owner():
        # No subsequent commands are supported. Parent close/crash means revoke ownership.
        try:
            while os.read(sys.stdin.fileno(), 1):
                pass
        except OSError:
            pass
        lost.set()
    threading.Thread(target=watch_owner, daemon=True).start()
    try:
        request = spec.get("input")
        if request is not None:
            child.stdin.write((json.dumps(request, ensure_ascii=False) + "\n").encode("utf-8"))
        child.stdin.close()
        while child.poll() is None and not lost.wait(0.05):
            pass
        return child.returncode if child.returncode is not None else 125
    finally:
        if close_job:
            close_job()
        else:
            try: os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError: pass
            time.sleep(0.1)
            try: os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError: pass
        try: child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill(); child.wait(timeout=5)


if __name__ == "__main__":
    try: sys.exit(main())
    except Exception:
        print("Owned process supervisor failed; inspect executable and platform support", file=sys.stderr)
        sys.exit(122)
