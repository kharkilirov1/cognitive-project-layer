#!/usr/bin/env python3
"""OpenCode-compatible MCP stdio wrapper for Cognitive Project Layer.

This wrapper uses the official Python MCP server runtime and delegates heavy work
to the compiled `cpl.exe` CLI. It exists because OpenCode on Windows reliably
connects to Python MCP stdio servers.
"""

from __future__ import annotations

import argparse
import asyncio
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from mcp.server import NotificationOptions, Server
from mcp.server.models import InitializationOptions
import mcp.server.stdio
from mcp.types import TextContent, Tool


parser = argparse.ArgumentParser()
parser.add_argument("--root", default=os.getcwd())
ARGS = parser.parse_args()
ROOT = Path(ARGS.root).resolve()
CPL_HOME = Path(__file__).resolve().parents[1]
CPL_MANIFEST = CPL_HOME / "Cargo.toml"

app = Server("cognitive-project-layer")


def run_cpl(args: list[str], timeout: int = 120) -> str:
    cpl_exe = resolve_cpl_exe()
    if cpl_exe:
        command = [str(cpl_exe), "--root", str(ROOT), *args]
    else:
        command = [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(CPL_MANIFEST),
            "--bin",
            "cpl",
            "--",
            "--root",
            str(ROOT),
            *args,
        ]

    result = subprocess.run(
        command,
        cwd=str(CPL_HOME),
        text=True,
        capture_output=True,
        timeout=timeout,
        env={**os.environ},
    )
    if result.returncode != 0:
        raise RuntimeError((result.stderr or result.stdout).strip())
    return result.stdout.strip()


def resolve_cpl_exe() -> Path | None:
    exe_name = "cpl.exe" if os.name == "nt" else "cpl"
    for candidate in [
        CPL_HOME / ".cpl" / "bin" / exe_name,
        Path.home() / ".cpl" / "bin" / exe_name,
        CPL_HOME / "target" / "release" / exe_name,
        CPL_HOME / "target" / "debug" / exe_name,
    ]:
        if candidate.exists():
            return candidate
    return None


def text(value: str) -> list[TextContent]:
    return [TextContent(type="text", text=value)]


@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="cpl_scan",
            description="Scan project: languages, configs, entry points, recent changes.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="cpl_skeleton",
            description="Return the always-on project skeleton prompt.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="cpl_retrieve",
            description="Hybrid retrieve project context for a coding-agent query.",
            inputSchema={
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            },
        ),
        Tool(
            name="cpl_context",
            description="Build managed LLM context for a query with token budget.",
            inputSchema={
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            },
        ),
        Tool(
            name="cpl_symbols",
            description="Find symbols by exact/fuzzy name.",
            inputSchema={
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "json": {"type": "boolean"},
                },
            },
        ),
        Tool(
            name="cpl_references",
            description="Find references/usages for a symbol.",
            inputSchema={
                "type": "object",
                "properties": {"symbol": {"type": "string"}},
                "required": ["symbol"],
            },
        ),
        Tool(
            name="cpl_embed_search",
            description="Search persistent local neural embedding DB.",
            inputSchema={
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100},
                },
                "required": ["query"],
            },
        ),
        Tool(
            name="cpl_build_embeddings",
            description="Build persistent embeddings DB. Defaults to local Ollama nomic-embed-text.",
            inputSchema={
                "type": "object",
                "properties": {
                    "backend": {"type": "string", "enum": ["ollama", "local-hash", "openai-compatible", "openai"]},
                    "model": {"type": "string"},
                    "dimensions": {"type": "integer", "minimum": 8},
                },
            },
        ),
        Tool(
            name="cpl_index_build",
            description="Build persistent structural SQLite index under .cpl/index.sqlite.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="cpl_index_db",
            description="Show persistent structural SQLite index summary.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="cpl_index_freshness",
            description="Check whether the persistent SQLite index is fresh.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="cpl_doctor",
            description="Diagnose CPL binaries, MCP config, SQLite index, vector DB, and Ollama.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="cpl_panel",
            description="Render CPL transparency/status panel.",
            inputSchema={
                "type": "object",
                "properties": {"query": {"type": "string"}},
            },
        ),
        Tool(
            name="cpl_tree",
            description="Ignored-aware project file tree.",
            inputSchema={
                "type": "object",
                "properties": {"depth": {"type": "integer", "minimum": 1, "maximum": 10}},
            },
        ),
        Tool(
            name="cpl_grep",
            description="Grep over project text with ignored folders excluded.",
            inputSchema={
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200},
                },
                "required": ["pattern"],
            },
        ),
    ]


@app.call_tool()
async def call_tool(name: str, arguments: dict[str, Any] | None):
    args = arguments or {}
    try:
        if name == "cpl_scan":
            return text(run_cpl(["scan"]))
        if name == "cpl_skeleton":
            return text(run_cpl(["skeleton"]))
        if name == "cpl_retrieve":
            return text(run_cpl(["retrieve", str(args["query"])]))
        if name == "cpl_context":
            return text(run_cpl(["context", str(args["query"])]))
        if name == "cpl_symbols":
            command = ["symbols"]
            if args.get("query"):
                command.append(str(args["query"]))
            if args.get("json"):
                command.append("--json")
            return text(run_cpl(command))
        if name == "cpl_references":
            return text(run_cpl(["references", str(args["symbol"])]))
        if name == "cpl_embed_search":
            return text(
                run_cpl(["embed-search", str(args["query"]), "--limit", str(args.get("limit", 10))])
            )
        if name == "cpl_build_embeddings":
            return text(
                run_cpl(
                    [
                        "embed-index",
                        "--backend",
                        str(args.get("backend", "ollama")),
                        "--model",
                        str(args.get("model", "nomic-embed-text")),
                        "--dimensions",
                        str(args.get("dimensions", 768)),
                    ],
                    timeout=600,
                )
            )
        if name == "cpl_index_build":
            return text(run_cpl(["index-build"], timeout=600))
        if name == "cpl_index_db":
            return text(run_cpl(["index-db"]))
        if name == "cpl_index_freshness":
            return text(run_cpl(["index-freshness"]))
        if name == "cpl_doctor":
            return text(run_cpl(["doctor"], timeout=180))
        if name == "cpl_panel":
            command = ["panel"]
            if args.get("query"):
                command.append(str(args["query"]))
            return text(run_cpl(command))
        if name == "cpl_tree":
            return text(run_cpl(["tree", "--depth", str(args.get("depth", 3))]))
        if name == "cpl_grep":
            return text(run_cpl(["grep", str(args["pattern"]), "--limit", str(args.get("limit", 30))]))
        return text(f"Unknown tool: {name}")
    except Exception as error:
        return text(f"Error: {error}")


async def main() -> None:
    print(f"CPL OpenCode MCP wrapper root={ROOT}", file=sys.stderr)
    async with mcp.server.stdio.stdio_server() as (read_stream, write_stream):
        await app.run(
            read_stream,
            write_stream,
            InitializationOptions(
                server_name="cognitive-project-layer",
                server_version="0.1.0",
                capabilities=app.get_capabilities(
                    notification_options=NotificationOptions(),
                    experimental_capabilities={},
                ),
            ),
        )


if __name__ == "__main__":
    asyncio.run(main())
