#!/usr/bin/env python3
"""Package/contract checks only. Not a Rust parser, compiler, or test runner.

Requires Python 3.11+ (stdlib only). Run from any current directory.
Use --output docs/STATIC-CHECK.json to save a report before packaging.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import unquote

try:
    import tomllib
except ModuleNotFoundError:
    sys.exit("preflight.py requires Python 3.11 or newer; it does not install dependencies.")

ROOT = Path(__file__).resolve().parents[1]
TEST = re.compile(r"#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)")


def check_package() -> dict:
    errors: list[str] = []
    checks: list[dict] = []

    def record(name: str, problems: list[str]) -> None:
        checks.append({"name": name, "passed": not problems, "errors": problems})
        errors.extend(problems)

    required = [
        "Cargo.toml", "rust-toolchain.toml", "README.md", "LICENSE",
        "docs/ARCHITECTURE.zh-CN.md", "docs/DESIGN-NOTES.zh-CN.md",
        "docs/PLUGIN-GUIDE.zh-CN.md", "docs/ACCEPTANCE.zh-CN.md",
        "docs/TEST-INVENTORY.md", "docs/DELIVERY.zh-CN.md", "docs/ADR.md",
        "docs/REFERENCES.md", "verification-environment.txt",
        "docs/BRIDGES.zh-CN.md", "docs/APPLICATION-API.zh-CN.md", "docs/STREAMING-PROTOCOL.zh-CN.md",
        "docs/MIGRATION-0.2.zh-CN.md", "docs/MIGRATION-0.3.zh-CN.md",
        "docs/GENERIC-CORE-BOUNDARY.zh-CN.md", "docs/CONTENT-AND-PROVIDERS.zh-CN.md", "docs/CHECKPOINTS.zh-CN.md", "docs/SECURITY.zh-CN.md", "clients/javascript/package.json",
        "scripts/verify.sh", "scripts/verify.ps1", ".github/workflows/ci.yml",
    ]
    record("required_files", [f"missing {path}" for path in required if not (ROOT / path).is_file()])

    manifests = {}
    problems = []
    for path in sorted(ROOT.rglob("*.toml")):
        if "target" in path.relative_to(ROOT).parts:
            continue
        try:
            manifests[path] = tomllib.loads(path.read_text(encoding="utf-8"))
        except (tomllib.TOMLDecodeError, UnicodeError) as exc:
            problems.append(f"{path.relative_to(ROOT)}: {exc}")
    record("toml_parsing", problems)
    workspace = manifests.get(ROOT / "Cargo.toml", {}).get("workspace", {})
    members = workspace.get("members", [])
    workspace_deps = workspace.get("dependencies", {})

    problems = []
    names = set()
    for member in members:
        path = ROOT / member / "Cargo.toml"
        if path not in manifests:
            problems.append(f"workspace member lacks valid Cargo.toml: {member}")
            continue
        package = manifests[path].get("package", {})
        name = package.get("name")
        if not name or name in names:
            problems.append(f"missing/duplicate package name: {member}")
        names.add(name)
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for dep, spec in manifests[path].get(section, {}).items():
                if not isinstance(spec, dict):
                    continue
                origin = path.parent
                if spec.get("workspace"):
                    if dep not in workspace_deps:
                        problems.append(f"{member}: missing workspace dependency {dep}")
                        continue
                    spec = workspace_deps[dep]
                    origin = ROOT
                if isinstance(spec, dict) and "path" in spec:
                    candidate = (origin / spec["path"] / "Cargo.toml").resolve()
                    if not candidate.is_relative_to(ROOT) or not candidate.is_file():
                        problems.append(f"{member}: missing/outside dependency path for {dep}")
        for binary in manifests[path].get("bin", []):
            if "path" in binary and not (path.parent / binary["path"]).is_file():
                problems.append(f"{member}: missing binary {binary}")
    record("workspace_paths_and_dependencies", problems)

    # Production edges only. Tests/composition roots are allowed to assemble implementations.
    paths_by_name = {m.get("package", {}).get("name"): path for path, m in manifests.items() if path.name == "Cargo.toml" and "package" in m}
    graph = {}
    for name, path in paths_by_name.items():
        edges = set()
        for dep, spec in manifests[path].get("dependencies", {}).items():
            if isinstance(spec, dict) and spec.get("workspace"):
                spec = workspace_deps.get(dep, {})
            actual = spec.get("package", dep) if isinstance(spec, dict) else dep
            if actual in paths_by_name: edges.add(actual)
        graph[name] = edges
    allowed = {
        "agent-api": set(), "agent-core": {"agent-api"}, "agent-providers": {"agent-api"},
        "agent-application": {"agent-api"}, "agent-memory": {"agent-api"}, "agent-planner": {"agent-api"},
        "agent-bridge-http": {"agent-api", "agent-application"},
        "tauri-plugin-agent-bridge": {"agent-api", "agent-application"},
    }
    problems = []
    for name, permitted in allowed.items():
        for dep in graph.get(name, set()) - permitted:
            problems.append(f"{name}: forbidden production dependency {dep}")
    record("production_dependency_direction", problems)
    problems = []
    for root_name in allowed:
        queue = list(graph.get(root_name, [])); reached = set()
        while queue:
            name = queue.pop()
            if name in reached: continue
            reached.add(name); queue.extend(graph.get(name, []))
        for dep in reached - allowed[root_name]:
            problems.append(f"{root_name}: forbidden transitive project dependency {dep}")
    record("production_transitive_decoupling", problems)
    tauri = manifests.get(ROOT / "bridges/tauri/Cargo.toml", {})
    record("tauri_native_is_explicitly_optional", [] if (
        tauri.get("features", {}).get("default") == []
        and tauri.get("dependencies", {}).get("tauri", {}).get("optional")
        and tauri.get("build-dependencies", {}).get("tauri-plugin", {}).get("optional")
        and not any(m.startswith("bridges/") for m in workspace.get("default-members", []))
    ) else ["native Tauri or bridge default selection is not optional"])
    problems = []
    for path in ROOT.rglob("*.json"):
        if any(part in {"target", "node_modules", ".git"} for part in path.relative_to(ROOT).parts): continue
        try: json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc: problems.append(f"{path.relative_to(ROOT)}: {exc}")
    record("json_files_parse", problems)

    sources = sorted(p for p in ROOT.rglob("*.rs") if "target" not in p.relative_to(ROOT).parts)
    tests = []
    problems = []
    module_problems = []
    for path in sources:
        text = path.read_text(encoding="utf-8")
        rel = path.relative_to(ROOT).as_posix()
        for match in TEST.finditer(text):
            tests.append({"file": rel, "test": match.group(1), "execution": "not_run_by_preflight"})
        if re.search(r"\b(?:todo|unimplemented)!\s*\(", text):
            problems.append(f"unfinished implementation macro in {rel}")
        # Only checks conventional external module files, not the Rust grammar.
        for match in re.finditer(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", text, re.MULTILINE):
            name = match.group(1)
            if not (path.parent / f"{name}.rs").is_file() and not (path.parent / name / "mod.rs").is_file():
                module_problems.append(f"{rel}: missing module file {name}")
    record("no_todo_or_unimplemented_macros", problems)
    record("conventional_external_module_paths", module_problems)

    inventory_path = ROOT / "docs/TEST-INVENTORY.md"
    inventory = inventory_path.read_text(encoding="utf-8") if inventory_path.is_file() else ""
    record("authored_test_inventory", [
        f"test missing from inventory: {item['file']}::{item['test']}"
        for item in tests if f"`{item['test']}`" not in inventory or f"`{item['file']}`" not in inventory
    ] + ([] if tests else ["no authored Rust tests found"]))

    problems = []
    for path in ROOT.rglob("*.md"):
        if "target" in path.relative_to(ROOT).parts:
            continue
        text = path.read_text(encoding="utf-8")
        for target in re.findall(r"\[[^\]]*\]\(([^)]+)\)", text):
            target = target.split()[0].strip("<>")
            if not target or target.startswith(("http:", "https:", "mailto:", "#")):
                continue
            destination = unquote(target.split("#", 1)[0])
            if destination and not (path.parent / destination).exists():
                problems.append(f"{path.relative_to(ROOT)}: missing link target {target}")
    record("local_markdown_link_targets", problems)

    manifest = ROOT / "MANIFEST.sha256"
    if manifest.is_file():
        problems = []
        listed = set()
        for line in manifest.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                expected, relative = line.split("  ", 1)
            except ValueError:
                problems.append("invalid SHA256 manifest line")
                continue
            listed.add(relative)
            path = (ROOT / relative).resolve()
            if not path.is_relative_to(ROOT) or not path.is_file():
                problems.append(f"manifest file missing/unsafe: {relative}")
            elif hashlib.sha256(path.read_bytes()).hexdigest() != expected:
                problems.append(f"manifest hash mismatch: {relative}")
        record("delivery_sha256_manifest", problems)
    else:
        checks.append({"name": "delivery_sha256_manifest", "passed": None,
                       "note": "not generated yet; hashes are generated after static report"})

    return {
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "status": "passed_static_structure_checks" if not errors else "failed_static_structure_checks",
        "scope": "file, TOML/JSON, production dependency graph, native feature selection, module path, docs, test inventory, optional hashes",
        "production_project_graph": {name: sorted(edges) for name, edges in graph.items()},
        "rust_compilation": "not_performed",
        "rust_test_execution": "not_performed",
        "rust_type_or_semantic_validation": "not_performed",
        "real_provider_acceptance": "not_performed",
        "workspace_members": members,
        "rust_source_files": len(sources),
        "rust_physical_lines": sum(len(p.read_text(encoding="utf-8").splitlines()) for p in sources),
        "authored_rust_tests": len(tests),
        "checks": checks,
        "errors": errors,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="write JSON report, relative to project root")
    args = parser.parse_args()
    report = check_package()
    serialized = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        destination = args.output if args.output.is_absolute() else ROOT / args.output
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(serialized, encoding="utf-8")
    print(serialized, end="")
    return 1 if report["errors"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
