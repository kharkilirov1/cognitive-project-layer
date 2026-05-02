#!/usr/bin/env python3
"""Measure warm MCP stdio latency for the CPL MCP server."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import threading
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
    parser.add_argument("--mcp", help="Path or command for the cpl-mcp server")
    parser.add_argument("--iterations", type=int, default=3, help="Measured warm iterations")
    parser.add_argument("--warmup", type=int, default=1, help="Warmup iterations")
    parser.add_argument("--json-out", help="Optional path for machine-readable results")
    return parser.parse_args()


def resolve_mcp_base(explicit: str | None) -> list[str] | None:
    if explicit:
        return split_command(explicit)

    env_bin = os.environ.get("CPL_MCP_BIN")
    if env_bin:
        return split_command(env_bin)

    exe_name = "cpl-mcp.exe" if os.name == "nt" else "cpl-mcp"
    candidates = [
        REPO_ROOT / ".cpl" / "verify-target" / "release" / exe_name,
        REPO_ROOT / ".cpl" / "verify-target" / "debug" / exe_name,
        REPO_ROOT / "target" / "release" / exe_name,
        REPO_ROOT / "target" / "debug" / exe_name,
    ]
    for candidate in candidates:
        if candidate.exists():
            return [str(candidate)]

    from_path = shutil.which("cpl-mcp")
    if from_path:
        return [from_path]

    return None


def command_for(base: list[str] | None, root: Path) -> list[str]:
    if base is None:
        return ["cargo", "run", "--quiet", "--bin", "cpl-mcp", "--", "--root", str(root)]
    return [*base, "--root", str(root)]


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


def root_label(path: Path) -> str:
    try:
        return path.resolve().relative_to(REPO_ROOT).as_posix() or "."
    except ValueError:
        return str(path)


class McpClient:
    def __init__(self, base: list[str] | None, root: Path) -> None:
        self.command = command_for(base, root)
        self.process = subprocess.Popen(
            self.command,
            cwd=REPO_ROOT,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.next_id = 1
        self.stderr_lines: list[str] = []
        self._stderr_thread = threading.Thread(target=self._drain_stderr, daemon=True)
        self._stderr_thread.start()

    def _drain_stderr(self) -> None:
        assert self.process.stderr is not None
        for line in self.process.stderr:
            text = line.decode("utf-8", errors="replace").rstrip()
            if len(self.stderr_lines) < 40:
                self.stderr_lines.append(text)

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        payload = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
        }
        if params is not None:
            payload["params"] = params
        self._write(payload)
        response = self._read()
        if response.get("id") != request_id:
            raise RuntimeError(f"Unexpected MCP response id: {response}")
        if response.get("error"):
            raise RuntimeError(f"MCP error: {response['error']}")
        return response

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        payload = {
            "jsonrpc": "2.0",
            "method": method,
        }
        if params is not None:
            payload["params"] = params
        self._write(payload)

    def call_tool(self, name: str, arguments: dict[str, Any] | None = None) -> str:
        response = self.request(
            "tools/call",
            {
                "name": name,
                "arguments": arguments or {},
            },
        )
        result = response.get("result", {})
        if result.get("isError"):
            content = result.get("content", [])
            text = content[0].get("text", "") if content else ""
            raise RuntimeError(f"MCP tool `{name}` failed: {text}")
        content = result.get("content", [])
        return content[0].get("text", "") if content else ""

    def _write(self, payload: dict[str, Any]) -> None:
        if self.process.poll() is not None:
            raise RuntimeError(
                "MCP process exited before request. "
                f"Command: {' '.join(self.command)}\n"
                f"stderr:\n{self.stderr_text()}"
            )
        assert self.process.stdin is not None
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        header = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii")
        self.process.stdin.write(header + body)
        self.process.stdin.flush()

    def _read(self) -> dict[str, Any]:
        assert self.process.stdout is not None
        content_length = None
        while True:
            line = self.process.stdout.readline()
            if line == b"":
                raise RuntimeError(
                    "MCP process closed stdout. "
                    f"Command: {' '.join(self.command)}\n"
                    f"stderr:\n{self.stderr_text()}"
                )
            line = line.rstrip(b"\r\n")
            if not line:
                break
            name, _, value = line.partition(b":")
            if name.lower() == b"content-length":
                content_length = int(value.strip())

        if content_length is None:
            raise RuntimeError("MCP response missing Content-Length")
        body = self.process.stdout.read(content_length)
        if len(body) != content_length:
            raise RuntimeError("MCP response ended before Content-Length bytes were read")
        return json.loads(body.decode("utf-8"))

    def stderr_text(self) -> str:
        return "\n".join(self.stderr_lines)

    def close(self) -> None:
        if self.process.poll() is not None:
            return
        if self.process.stdin is not None:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)


def timed(callable_obj) -> tuple[float, Any]:
    started = time.perf_counter()
    value = callable_obj()
    return (time.perf_counter() - started) * 1000.0, value


def measure_tool(
    client: McpClient,
    name: str,
    arguments: dict[str, Any],
    iterations: int,
    warmup: int,
) -> list[float]:
    for _ in range(warmup):
        client.call_tool(name, arguments)

    durations = []
    for _ in range(iterations):
        duration, _ = timed(lambda: client.call_tool(name, arguments))
        durations.append(duration)
    return durations


def append_record(
    records: list[dict[str, Any]],
    operation: str,
    target: str,
    durations: list[float],
    iterations: int,
    extra: dict[str, Any] | None = None,
) -> None:
    record = {
        "operation": operation,
        "target": target,
        "iterations": iterations,
        "p50_ms": percentile(durations, 50),
        "p95_ms": percentile(durations, 95),
        "min_ms": min(durations),
        "max_ms": max(durations),
    }
    if extra:
        record.update(extra)
    records.append(record)


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
    base = resolve_mcp_base(args.mcp)

    roots = {(REPO_ROOT / args.root).resolve()}
    cases_by_root: dict[Path, list[dict[str, Any]]] = {}
    for case in cases:
        root = (REPO_ROOT / case["root"]).resolve()
        roots.add(root)
        cases_by_root.setdefault(root, []).append(case)

    records: list[dict[str, Any]] = []
    for root in sorted(roots, key=root_label):
        client = McpClient(base, root)
        try:
            init_duration, _ = timed(
                lambda: client.request(
                    "initialize",
                    {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name": "cpl-bench-mcp", "version": "0.1.0"},
                    },
                )
            )
            client.notify("notifications/initialized")
            append_record(
                records,
                "initialize",
                root_label(root),
                [init_duration],
                1,
            )

            tools_duration, _ = timed(lambda: client.request("tools/list"))
            append_record(records, "tools/list", root_label(root), [tools_duration], 1)

            cold_duration, _ = timed(lambda: client.call_tool("cpl_skeleton"))
            append_record(records, "layer_init", root_label(root), [cold_duration], 1)

            for tool_name in ("cpl_scan", "cpl_skeleton"):
                durations = measure_tool(client, tool_name, {}, args.iterations, args.warmup)
                append_record(records, tool_name, root_label(root), durations, args.iterations)

            for case in cases_by_root.get(root, []):
                durations = measure_tool(
                    client,
                    "cpl_retrieve",
                    {"query": case["query"]},
                    args.iterations,
                    args.warmup,
                )
                append_record(
                    records,
                    "cpl_retrieve",
                    case["id"],
                    durations,
                    args.iterations,
                    {"root": root_label(root)},
                )
        finally:
            client.close()

    base_label = "cargo run --bin cpl-mcp" if base is None else " ".join(base)
    print(f"MCP: {base_label}")
    print(f"Iterations: {args.iterations} measured, {args.warmup} warmup")
    print()
    print(f"{'operation':14} {'target':40} {'p50 ms':>10} {'p95 ms':>10} {'min ms':>10} {'max ms':>10}")
    print("-" * 104)
    for record in records:
        print(
            f"{record['operation'][:14]:14} "
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
        print(f"bench_mcp.py: error: {exc}", file=sys.stderr)
        raise SystemExit(2)
