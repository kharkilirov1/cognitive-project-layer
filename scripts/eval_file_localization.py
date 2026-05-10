#!/usr/bin/env python3
"""Evaluate whether CPL localizes expected files in top-k retrieval results."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", default="evals/retrieval.json")
    parser.add_argument("--cpl", default=None)
    parser.add_argument("--top-k", type=int, default=5)
    parser.add_argument("--json-out", default=".cpl/eval-results/file-localization.json")
    return parser.parse_args()


def resolve_cpl(explicit: str | None) -> str:
    if explicit:
        return explicit
    for candidate in [Path("target/release/cpl.exe"), Path("target/debug/cpl.exe"), Path("target/release/cpl"), Path("target/debug/cpl")]:
        if candidate.exists():
            return str(candidate)
    found = shutil.which("cpl")
    if found:
        return found
    raise SystemExit("cpl binary not found; build it or pass --cpl")


def main() -> int:
    args = parse_args()
    cpl = resolve_cpl(args.cpl)
    cases = json.loads(Path(args.cases).read_text(encoding="utf-8"))
    records = []
    for case in cases:
        completed = subprocess.run(
            [cpl, "--root", case["root"], "retrieve", case["query"], "--json"],
            check=True,
            capture_output=True,
            text=True,
        )
        data = json.loads(completed.stdout)
        top_paths = []
        for chunk in data.get("chunks", []):
            path = chunk.get("path", "").replace("\\", "/")
            if path not in top_paths:
                top_paths.append(path)
        expected = [path.replace("\\", "/") for path in case.get("expected_files", [])]
        ranks = []
        for expected_path in expected:
            rank = next((index for index, path in enumerate(top_paths, 1) if path == expected_path), None)
            ranks.append(rank)
        hit = any(rank is not None and rank <= args.top_k for rank in ranks)
        records.append(
            {
                "case": case["id"],
                "root": case["root"],
                "expected_files": expected,
                "ranks": ranks,
                "hit": hit,
                "top_paths": top_paths[: args.top_k],
            }
        )
        print(f"{'PASS' if hit else 'FAIL'} {case['id']} ranks={ranks} top={top_paths[:args.top_k]}")

    total = len(records)
    hits = sum(1 for record in records if record["hit"])
    mrr = sum((1 / min(rank for rank in record["ranks"] if rank)) if any(record["ranks"]) else 0.0 for record in records) / total
    result = {
        "benchmark": "fixture file localization",
        "top_k": args.top_k,
        "summary": {
            "passed": hits,
            "total": total,
            "file_hit_rate": hits / total,
            "mrr": mrr,
        },
        "records": records,
    }
    output = Path(args.json_out)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result["summary"], indent=2))
    print(f"saved {output}")
    return 0 if hits == total else 1


if __name__ == "__main__":
    raise SystemExit(main())
