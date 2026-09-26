from __future__ import annotations

import json
from pathlib import Path

from harness.common import HarnessError, read_artifact, safe_path


def json_equal(actual, expected) -> bool:
    # JSON object key order is irrelevant; boolean true is not integer 1.
    if type(actual) is not type(expected):
        return False
    if isinstance(actual, dict):
        return actual.keys() == expected.keys() and all(json_equal(actual[k], expected[k]) for k in actual)
    if isinstance(actual, list):
        return len(actual) == len(expected) and all(json_equal(a, b) for a, b in zip(actual, expected))
    return actual == expected


def grade(task: dict, observations: list, workspace: Path) -> list[dict]:
    results = []
    for index, check in enumerate(task["checks"]):
        result = {"index": index, "type": check["type"], "required": check.get("required", True), "status": "failed"}
        try:
            selected = check.get("turn", len(observations) - 1)
            if not 0 <= selected < len(observations):
                result.update(status="inconclusive", reason="The required turn did not complete")
                results.append(result); continue
            observation = observations[selected]
            kind = check["type"]
            passed = False
            if kind == "exact": passed = observation.output.strip() == check["value"].strip()
            elif kind == "contains": passed = check["value"] in observation.output
            elif kind == "json": passed = json_equal(json.loads(observation.output), check["value"])
            elif kind == "file_exact": passed = read_artifact(workspace, check["path"]) == check["value"]
            elif kind == "file_json": passed = json_equal(json.loads(read_artifact(workspace, check["path"])), check["value"])
            elif kind == "absent": passed = not safe_path(workspace, check["path"]).exists()
            elif kind == "unchanged": passed = read_artifact(workspace, check["path"]) == task["fixtures"][check["path"]]
            elif kind == "tool_count":
                if not observation.tool_events_complete:
                    result.update(status="inconclusive", reason="Complete tool execution evidence is unavailable")
                    results.append(result); continue
                passed = sum(t["name"] == check["name"] and t["status"] == "success" for t in observation.tools) == check["count"]
            else: raise HarnessError("invalid", "Unknown grader")
            result.update(status="passed" if passed else "failed", reason="Matched" if passed else "Expected condition was not satisfied")
        except (HarnessError, ValueError, OSError) as exc:
            result["reason"] = str(exc)[:512]
        results.append(result)
    return results


def verdict(checks: list[dict]) -> str:
    required = [c for c in checks if c["required"]]
    if not required: return "inconclusive"
    if any(c["status"] == "failed" for c in required): return "failed"
    if any(c["status"] != "passed" for c in required): return "inconclusive"
    return "passed"
