#!/usr/bin/env python3
"""Measure CPL CLI latency on eval fixtures and the current repository."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
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
    parser.add_argument("--root", default=".", help="Additional project root to benchmark")
    parser.add_argument("--cases", default="evals/retrieval.json", help="Retrieval cases JSON")
    parser.add_argument("--cpl", help="Path or command for the cpl binary")
    parser.add_argument("--iterations", type=int, default=3, help="Measured iterations")
    parser.add_argument("--warmup", type=int, default=1, help="Warmup iterations")
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


def command_for(base: list[str] | None, args: list[str]) -> list[str]:
    if base is None:
        return ["cargo", "run", "--quiet", "--bin", "cpl", "--", *args]
    return [*base, *args]


def run_command(base: list[str] | None, args: list[str]) -> None:
    command = command_for(base, args)
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


def load_cases(path: Path) -> list[dict[str, Any]]:
    with path.open("r", encoding="utf-8") as handle:
        data = json.load(handle)
    if not isinstance(data, list):
        raise ValueError(f"Expected a JSON list in {path}")
    return data


def percentile(values: list[float], p: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    rank = (len(ordered) - 1) * (p / 100.0)
    lower = int(rank)
    upper = min(lower + 1, len(ordered) - 1)
    weight = rank - lower
    return ordered[lower] * (1 - weight) + ordered[upper] * weight


def measure(base: list[str] | None, args: list[str], iterations: int, warmup: int) -> list[float]:
    for _ in range(warmup):
        run_command(base, args)

    durations = []
    for _ in range(iterations):
        started = time.perf_counter()
        run_command(base, args)
        durations.append((time.perf_counter() - started) * 1000.0)
    return durations


def root_label(path: Path) -> str:
    try:
        return path.resolve().relative_to(REPO_ROOT).as_posix() or "."
    except ValueError:
        return str(path)


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()
    if args.iterations < 1:
        raise ValueError("--iterations must be >= 1")
    if args.warmup < 0:
        raise ValueError("--warmup must be >= 0")

    cases_path = (REPO_ROOT / args.cases).resolve()
    cases = load_cases(cases_path)
    base = resolve_cpl_base(args.cpl)

    roots = {(REPO_ROOT / args.root).resolve()}
    for case in cases:
        roots.add((REPO_ROOT / case["root"]).resolve())

    records: list[dict[str, Any]] = []
    for root in sorted(roots, key=lambda item: root_label(item)):
        for operation in ("scan", "skeleton"):
            durations = measure(
                base,
                ["--root", str(root), operation, "--json"],
                args.iterations,
                args.warmup,
            )
            records.append(
                {
                    "operation": operation,
                    "target": root_label(root),
                    "iterations": args.iterations,
                    "p50_ms": percentile(durations, 50),
                    "p95_ms": percentile(durations, 95),
                    "min_ms": min(durations),
                    "max_ms": max(durations),
                }
            )

    for case in cases:
        root = (REPO_ROOT / case["root"]).resolve()
        durations = measure(
            base,
            ["--root", str(root), "retrieve", "--json", case["query"]],
            args.iterations,
            args.warmup,
        )
        records.append(
            {
                "operation": "retrieve",
                "target": case["id"],
                "root": root_label(root),
                "iterations": args.iterations,
                "p50_ms": percentile(durations, 50),
                "p95_ms": percentile(durations, 95),
                "min_ms": min(durations),
                "max_ms": max(durations),
            }
        )

    base_label = "cargo run --bin cpl" if base is None else " ".join(base)
    print(f"CPL: {base_label}")
    print(f"Iterations: {args.iterations} measured, {args.warmup} warmup")
    print()
    print(f"{'operation':10} {'target':40} {'p50 ms':>10} {'p95 ms':>10} {'min ms':>10} {'max ms':>10}")
    print("-" * 100)
    for record in records:
        print(
            f"{record['operation'][:10]:10} "
            f"{record['target'][:40]:40} "
            f"{record['p50_ms']:10.1f} "
            f"{record['p95_ms']:10.1f} "
            f"{record['min_ms']:10.1f} "
            f"{record['max_ms']:10.1f}"
        )

    payload = {
        "iterations": args.iterations,
        "warmup": args.warmup,
        "records": records,
    }
    if args.json_out:
        write_json((REPO_ROOT / args.json_out).resolve(), payload)

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"bench_cli.py: error: {exc}", file=sys.stderr)
        raise SystemExit(2)
