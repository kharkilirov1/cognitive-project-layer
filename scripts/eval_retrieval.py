#!/usr/bin/env python3
"""Run fixture-based retrieval quality evals for the CPL CLI."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]


def split_command(value: str) -> list[str]:
    expanded = os.path.expandvars(os.path.expanduser(value))
    direct_path = Path(expanded)
    repo_path = REPO_ROOT / expanded
    if direct_path.exists():
        return [str(direct_path)]
    if repo_path.exists():
        return [str(repo_path)]
    return shlex.split(value, posix=os.name != "nt")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", default="evals/retrieval.json", help="Path to eval case JSON")
    parser.add_argument("--cpl", help="Path or command for the cpl binary")
    parser.add_argument("--top-k", type=int, default=3, help="How many retrieved chunks count as a hit")
    parser.add_argument("--json-out", help="Optional path for machine-readable results")
    return parser.parse_args()


def resolve_cpl_base(explicit: str | None) -> list[str] | None:
    if explicit:
        return split_command(explicit)

    env_bin = os.environ.get("CPL_BIN")
    if env_bin:
        return split_command(env_bin)

    exe_name = "cpl.exe" if os.name == "nt" else "cpl"
    candidates = [
        REPO_ROOT / ".cpl" / "verify-target" / "debug" / exe_name,
        REPO_ROOT / "target" / "debug" / exe_name,
    ]
    for candidate in candidates:
        if candidate.exists():
            return [str(candidate)]

    from_path = shutil.which("cpl")
    if from_path:
        return [from_path]

    return None


def run_cpl(base: list[str] | None, args: list[str]) -> dict[str, Any]:
    if base is None:
        command = ["cargo", "run", "--quiet", "--bin", "cpl", "--", *args]
    else:
        command = [*base, *args]

    completed = subprocess.run(
        command,
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "CPL command failed with exit code "
            f"{completed.returncode}: {' '.join(command)}\n{completed.stderr.strip()}"
        )
    return json.loads(completed.stdout)


def load_cases(path: Path) -> list[dict[str, Any]]:
    with path.open("r", encoding="utf-8") as handle:
        data = json.load(handle)
    if not isinstance(data, list):
        raise ValueError(f"Expected a JSON list in {path}")
    return data


def normalize_path(value: str) -> str:
    return value.replace("\\", "/").strip("/").lower()


def path_matches(candidate: str, expected: str) -> bool:
    candidate_norm = normalize_path(candidate)
    expected_norm = normalize_path(expected)
    return (
        candidate_norm == expected_norm
        or candidate_norm.endswith("/" + expected_norm)
        or candidate_norm.endswith(expected_norm)
    )


def chunk_contains_symbol(chunk: dict[str, Any], symbol: str) -> bool:
    expected = symbol.lower()
    symbols = [str(item).lower() for item in chunk.get("symbols", [])]
    if expected in symbols:
        return True
    preview = str(chunk.get("preview", "")).lower()
    return expected in preview


def evaluate_case(base: list[str] | None, case: dict[str, Any], top_k: int) -> dict[str, Any]:
    root = str((REPO_ROOT / case["root"]).resolve())
    query = case["query"]
    result = run_cpl(base, ["--root", root, "retrieve", "--json", query])
    chunks = result.get("chunks", [])[:top_k]
    paths = [str(chunk.get("path", "")) for chunk in chunks]

    expected_files = case.get("expected_files", [])
    file_hit = all(any(path_matches(path, expected) for path in paths) for expected in expected_files)

    expected_symbols = case.get("expected_symbols", [])
    symbol_hit = all(
        any(chunk_contains_symbol(chunk, expected) for chunk in chunks)
        for expected in expected_symbols
    )

    confidence = float(result.get("confidence", 0.0) or 0.0)
    min_confidence = float(case.get("min_confidence", 0.0) or 0.0)
    confidence_hit = confidence >= min_confidence

    passed = bool(file_hit and symbol_hit and confidence_hit)
    return {
        "id": case["id"],
        "query": query,
        "root": case["root"],
        "passed": passed,
        "file_hit": file_hit,
        "symbol_hit": symbol_hit,
        "confidence_hit": confidence_hit,
        "confidence": confidence,
        "min_confidence": min_confidence,
        "top_k": top_k,
        "top_paths": paths,
        "strategy": result.get("strategy"),
    }


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()
    cases_path = (REPO_ROOT / args.cases).resolve()
    cases = load_cases(cases_path)
    base = resolve_cpl_base(args.cpl)

    results = [evaluate_case(base, case, args.top_k) for case in cases]
    passed = sum(1 for result in results if result["passed"])
    file_hits = sum(1 for result in results if result["file_hit"])
    symbol_hits = sum(1 for result in results if result["symbol_hit"])
    confidence_hits = sum(1 for result in results if result["confidence_hit"])
    avg_confidence = sum(result["confidence"] for result in results) / max(len(results), 1)

    base_label = "cargo run --bin cpl" if base is None else " ".join(base)
    print(f"CPL: {base_label}")
    print(f"Cases: {cases_path.relative_to(REPO_ROOT)} | top_k={args.top_k}")
    print()
    print(f"{'case':28} {'pass':5} {'file':5} {'symbol':7} {'conf':>7}  top paths")
    print("-" * 100)
    for result in results:
        top_paths = ", ".join(result["top_paths"][:3])
        print(
            f"{result['id'][:28]:28} "
            f"{'yes' if result['passed'] else 'no':5} "
            f"{'yes' if result['file_hit'] else 'no':5} "
            f"{'yes' if result['symbol_hit'] else 'no':7} "
            f"{result['confidence']:7.3f}  "
            f"{top_paths}"
        )

    print()
    print(
        "Summary: "
        f"passed={passed}/{len(results)}, "
        f"file@{args.top_k}={file_hits}/{len(results)}, "
        f"symbol@{args.top_k}={symbol_hits}/{len(results)}, "
        f"confidence={confidence_hits}/{len(results)}, "
        f"avg_confidence={avg_confidence:.3f}"
    )

    payload = {
        "top_k": args.top_k,
        "summary": {
            "passed": passed,
            "total": len(results),
            "file_hits": file_hits,
            "symbol_hits": symbol_hits,
            "confidence_hits": confidence_hits,
            "avg_confidence": avg_confidence,
        },
        "results": results,
    }
    if args.json_out:
        write_json((REPO_ROOT / args.json_out).resolve(), payload)

    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"eval_retrieval.py: error: {exc}", file=sys.stderr)
        raise SystemExit(2)
