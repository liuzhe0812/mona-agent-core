from pathlib import Path
import time

ROOT = Path(__file__).resolve().parents[1]


def wait_run(manager, rid, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = manager.store.get(rid)
        if result['status'] not in ('queued', 'running', 'cancelling'):
            # Durable report is published before the coordinator drops its active slot.
            with manager.lock:
                done = rid not in manager.active
            if done:
                return result
        time.sleep(.02)
    raise AssertionError('Evaluation failed to settle in time')


def base_request(**kwargs):
    return dict(adapter='selftest', model='fixture', suite='all', **kwargs)
