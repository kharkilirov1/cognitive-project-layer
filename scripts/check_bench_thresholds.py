#!/usr/bin/env python3
"""Fail CI when benchmark JSON exceeds configured latency thresholds."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


DEFAULT_THRESHOLDS_MS = {
    "cold_scan": 5_000.0,
    "cold_skeleton_no_index": 90_000.0,
    "index_build": 60_000.0,
    "index_freshness": 5_000.0,
    "index_refresh_unchanged": 5_000.0,
    "index_refresh_one_file": 10_000.0,
    "index_search_fts": 5_000.0,
    "warm_skeleton_sqlite": 10_000.0,
    "warm_retrieve_sqlite": 15_000.0,
    "embed_index_local_hash": 30_000.0,
    "embed_refresh_unchanged": 10_000.0,
    "embed_refresh_one_file": 15_000.0,
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, help="Benchmark JSON file produced by bench_large_repo.py")
    parser.add_argument(
        "--threshold",
        action="append",
        default=[],
        metavar="OPERATION:MS",
        help="Override/add a p95 threshold, e.g. warm_retrieve_sqlite:5000",
    )
    return parser.parse_args()


def parse_thresholds(values: list[str]) -> dict[str, float]:
    thresholds = dict(DEFAULT_THRESHOLDS_MS)
    for value in values:
        if ":" not in value:
            raise ValueError(f"invalid threshold `{value}`; expected OPERATION:MS")
        operation, raw_ms = value.split(":", 1)
        thresholds[operation.strip()] = float(raw_ms)
    return thresholds


def main() -> int:
    args = parse_args()
    path = Path(args.input)
    data = json.loads(path.read_text(encoding="utf-8"))
    thresholds = parse_thresholds(args.threshold)
    failures = []

    print(f"Benchmark threshold check: {path}")
    for record in data.get("records", []):
        operation = record.get("operation")
        if operation not in thresholds:
            continue
        p95 = float(record.get("p95_ms", 0.0))
        threshold = thresholds[operation]
        status = "PASS" if p95 <= threshold else "FAIL"
        print(f"{status:4} {operation:26} p95={p95:10.1f} ms threshold={threshold:10.1f} ms")
        if p95 > threshold:
            failures.append((operation, p95, threshold))

    if failures:
        print("\nBenchmark regressions detected:", file=sys.stderr)
        for operation, p95, threshold in failures:
            print(f"- {operation}: p95 {p95:.1f} ms > {threshold:.1f} ms", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"check_bench_thresholds.py: error: {exc}", file=sys.stderr)
        raise SystemExit(2)
