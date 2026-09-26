from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import sys
import threading
import time
import webbrowser

from .common import HarnessError
from .engine import Manager


def main() -> int:
    parser = argparse.ArgumentParser(description="Independent local Agent Evaluation Harness")
    parser.add_argument("--data", type=Path, default=Path(os.environ.get("EVAL_DATA_DIR", Path(__file__).resolve().parents[1] / ".local")))
    parser.add_argument("--config", type=Path, default=Path(os.environ["EVAL_CONFIG"]) if os.environ.get("EVAL_CONFIG") else None)
    sub = parser.add_subparsers(dest="command", required=True)
    serve = sub.add_parser("serve")
    serve.add_argument("--port", type=int, default=4318)
    serve.add_argument("--open", action="store_true")
    sub.add_parser("catalog")
    run = sub.add_parser("run")
    run.add_argument("--adapter", default="selftest")
    run.add_argument("--model", default="fixture")
    run.add_argument("--suite", default="all")
    run.add_argument("--repeats", type=int, default=1)
    run.add_argument("--concurrency", type=int, default=1)
    run.add_argument("--timeout", type=int, default=120)
    run.add_argument("--max-model-calls", type=int, default=16)
    run.add_argument("--max-tokens", type=int, default=32000)
    run.add_argument("--allow-paid", action="store_true")
    run.add_argument("--allow-local-execution", action="store_true")
    run.add_argument("--label", default="")
    report = sub.add_parser("report")
    report.add_argument("id")
    compare = sub.add_parser("compare")
    compare.add_argument("left")
    compare.add_argument("right")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    manager = Manager(root, args.data, args.config)
    try:
        if args.command == "serve":
            if not 0 <= args.port <= 65535:
                raise HarnessError("invalid", "Port must be 0..65535")
            from server.app import create_server
            try:
                server = create_server(manager, args.port)
            except OSError as exc:
                raise HarnessError("port", "Port unavailable; use --port with another port. No other service was stopped.") from exc
            url = f"http://127.0.0.1:{server.server_port}"
            print("Eval Harness: " + url, flush=True)
            print("本地可信执行；Ctrl+C 停止。自检不代表真实模型能力。", flush=True)

            def stop(*_):
                threading.Thread(target=server.shutdown, daemon=True).start()

            signal.signal(signal.SIGINT, stop)
            if hasattr(signal, "SIGTERM"):
                signal.signal(signal.SIGTERM, stop)
            if args.open:
                webbrowser.open(url)
            try:
                server.serve_forever(poll_interval=.1)
            finally:
                server.server_close()
            return 0
        if args.command == "catalog":
            print(json.dumps(manager.catalog.public(), ensure_ascii=False, indent=2))
            return 0
        if args.command == "report":
            print(json.dumps(manager.store.get(args.id), ensure_ascii=False, indent=2))
            return 0
        if args.command == "compare":
            print(json.dumps(manager.compare(args.left, args.right), ensure_ascii=False, indent=2))
            return 0
        request = {"adapter": args.adapter, "model": args.model, "suite": args.suite, "repeats": args.repeats,
                   "concurrency": args.concurrency, "timeout_s": args.timeout, "max_model_calls": args.max_model_calls,
                   "max_tokens": args.max_tokens, "allow_paid": args.allow_paid,
                   "allow_local_execution": args.allow_local_execution, "label": args.label}
        result = manager.start(request)
        rid = result["id"]

        def cancel(*_):
            manager.cancel(rid)

        signal.signal(signal.SIGINT, cancel)
        if hasattr(signal, "SIGTERM"):
            signal.signal(signal.SIGTERM, cancel)
        while True:
            result = manager.store.get(rid)
            if result["status"] not in ("queued", "running", "cancelling"):
                break
            time.sleep(.1)
        print(json.dumps({"id": rid, "mode": result["mode"], "status": result["status"], "summary": result["summary"], "data": str(manager.store.root)}, ensure_ascii=False, indent=2))
        return 0 if result["status"] == "completed" else 1
    finally:
        manager.close()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (HarnessError, OSError) as exc:
        print(f"Eval Harness: {exc}", file=sys.stderr)
        sys.exit(2)
