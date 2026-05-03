#!/usr/bin/env python3
"""Synthetic large-repository benchmark for CPL cold scan, SQLite build, and warm start."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Callable


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
    parser.add_argument("--files", type=int, default=1000, help="Number of Rust source files to generate")
    parser.add_argument("--symbols-per-file", type=int, default=3, help="Functions per generated file")
    parser.add_argument("--iterations", type=int, default=3, help="Measured iterations")
    parser.add_argument("--warmup", type=int, default=1, help="Warmup iterations")
    parser.add_argument("--cpl", help="Path or command for the cpl binary")
    parser.add_argument("--keep", action="store_true", help="Keep generated repository")
    parser.add_argument("--json-out", help="Optional path for machine-readable results")
    return parser.parse_args()


def resolve_cpl_base(explicit: str | None) -> list[str] | None:
    if explicit:
        return split_command(explicit)
    env_bin = os.environ.get("CPL_BIN")
    if env_bin:
        return split_command(env_bin)
    exe_name = "cpl.exe" if os.name == "nt" else "cpl"
    for candidate in [
        REPO_ROOT / ".cpl" / "verify-target" / "release" / exe_name,
        REPO_ROOT / ".cpl" / "verify-target" / "debug" / exe_name,
        REPO_ROOT / "target" / "release" / exe_name,
        REPO_ROOT / "target" / "debug" / exe_name,
    ]:
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


def run_command(base: list[str] | None, args: list[str], cwd: Path) -> None:
    command = command_for(base, args)
    completed = subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "CPL command failed with exit code "
            f"{completed.returncode}: {' '.join(command)}\n{completed.stderr.strip()}\n{completed.stdout.strip()}"
        )


def measure(base: list[str] | None, args: list[str], cwd: Path, iterations: int, warmup: int) -> list[float]:
    for _ in range(warmup):
        run_command(base, args, cwd)
    durations = []
    for _ in range(iterations):
        started = time.perf_counter()
        run_command(base, args, cwd)
        durations.append((time.perf_counter() - started) * 1000.0)
    return durations


def measure_with_hook(
    base: list[str] | None,
    args: list[str],
    cwd: Path,
    iterations: int,
    warmup: int,
    before_each: Callable[[int], None],
) -> list[float]:
    tick = 0
    for _ in range(warmup):
        before_each(tick)
        tick += 1
        run_command(base, args, cwd)
    durations = []
    for _ in range(iterations):
        before_each(tick)
        tick += 1
        started = time.perf_counter()
        run_command(base, args, cwd)
        durations.append((time.perf_counter() - started) * 1000.0)
    return durations


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


def generate_repo(root: Path, files: int, symbols_per_file: int) -> None:
    (root / "src" / "modules").mkdir(parents=True)
    (root / ".cplignore").write_text("target/\n.cpl/\n", encoding="utf-8")
    (root / "Cargo.toml").write_text(
        '[package]\nname="cpl-large-synthetic"\nversion="0.1.0"\nedition="2024"\n',
        encoding="utf-8",
    )
    lib = []
    for idx in range(files):
        module_name = f"module_{idx:05d}"
        lib.append(f"pub mod {module_name};")
        body = [f"// synthetic module {idx}"]
        if idx > 0:
            body.append(f"use super::module_{idx - 1:05d}::feature_{idx - 1:05d}_0;")
        for sym in range(symbols_per_file):
            body.append(
                f"pub fn feature_{idx:05d}_{sym}(input: usize) -> usize {{ input + {idx} + {sym} }}"
            )
        if idx > 0:
            body.append(
                f"pub fn linked_feature_{idx:05d}(input: usize) -> usize {{ feature_{idx - 1:05d}_0(input) }}"
            )
        (root / "src" / "modules" / f"{module_name}.rs").write_text("\n".join(body) + "\n", encoding="utf-8")
    (root / "src" / "modules" / "mod.rs").write_text("\n".join(lib) + "\n", encoding="utf-8")
    (root / "src" / "lib.rs").write_text("pub mod modules;\n", encoding="utf-8")


def mutate_one_source_file(root: Path, tick: int) -> None:
    path = root / "src" / "modules" / "module_00000.rs"
    text = path.read_text(encoding="utf-8").split("// refresh marker", 1)[0].rstrip()
    path.write_text(f"{text}\n// refresh marker {tick}\n", encoding="utf-8")


def record(operation: str, durations: list[float], extra: dict[str, Any] | None = None) -> dict[str, Any]:
    item = {
        "operation": operation,
        "iterations": len(durations),
        "p50_ms": percentile(durations, 50),
        "p95_ms": percentile(durations, 95),
        "min_ms": min(durations),
        "max_ms": max(durations),
    }
    if extra:
        item.update(extra)
    return item


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()
    if args.files < 1:
        raise ValueError("--files must be >= 1")
    if args.symbols_per_file < 1:
        raise ValueError("--symbols-per-file must be >= 1")
    if args.iterations < 1:
        raise ValueError("--iterations must be >= 1")

    base = resolve_cpl_base(args.cpl)
    root = Path(tempfile.mkdtemp(prefix="cpl-large-synthetic-"))
    try:
        generate_repo(root, args.files, args.symbols_per_file)
        records = []
        records.append(record("cold_scan", measure(base, ["--root", str(root), "scan", "--json"], REPO_ROOT, args.iterations, args.warmup)))
        records.append(record("cold_skeleton_no_index", measure(base, ["--root", str(root), "skeleton", "--json"], REPO_ROOT, args.iterations, args.warmup)))
        records.append(record("index_build", measure(base, ["--root", str(root), "index-build", "--json"], REPO_ROOT, args.iterations, 0)))
        records.append(record("index_freshness", measure(base, ["--root", str(root), "index-freshness", "--json"], REPO_ROOT, args.iterations, args.warmup)))
        records.append(record("index_refresh_unchanged", measure(base, ["--root", str(root), "index-refresh", "--json"], REPO_ROOT, args.iterations, args.warmup)))
        records.append(record("index_refresh_one_file", measure_with_hook(
            base,
            ["--root", str(root), "index-refresh", "--json"],
            REPO_ROOT,
            args.iterations,
            args.warmup,
            lambda tick: mutate_one_source_file(root, tick),
        )))
        records.append(record("warm_skeleton_sqlite", measure(base, ["--root", str(root), "skeleton", "--json"], REPO_ROOT, args.iterations, args.warmup)))
        records.append(record("warm_retrieve_sqlite", measure(base, ["--root", str(root), "retrieve", "--json", "feature 42 linked"], REPO_ROOT, args.iterations, args.warmup)))

        print(f"Synthetic repo: {root}")
        print(f"Files: {args.files}; symbols_per_file: {args.symbols_per_file}")
        print(f"{'operation':24} {'p50 ms':>10} {'p95 ms':>10} {'min ms':>10} {'max ms':>10}")
        print("-" * 72)
        for item in records:
            print(
                f"{item['operation'][:24]:24} "
                f"{item['p50_ms']:10.1f} "
                f"{item['p95_ms']:10.1f} "
                f"{item['min_ms']:10.1f} "
                f"{item['max_ms']:10.1f}"
            )
        payload = {
            "root": str(root),
            "files": args.files,
            "symbols_per_file": args.symbols_per_file,
            "iterations": args.iterations,
            "warmup": args.warmup,
            "records": records,
        }
        if args.json_out:
            write_json((REPO_ROOT / args.json_out).resolve(), payload)
        return 0
    finally:
        if args.keep:
            print(f"Kept synthetic repo: {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"bench_large_repo.py: error: {exc}", file=sys.stderr)
        raise SystemExit(2)
