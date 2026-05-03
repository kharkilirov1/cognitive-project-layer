# Evals and benchmarks

CPL has a small, deterministic eval layer for checking whether retrieval keeps
finding the right files and symbols as the project evolves.

## Retrieval eval

The retrieval eval uses fixture projects under `evals/fixtures/` and cases in
`evals/retrieval.json`.

```powershell
cargo build --bins
python scripts/eval_retrieval.py --cpl .\target\debug\cpl.exe --top-k 3
```

If no `--cpl` is supplied, the script tries `CPL_BIN`, local debug binaries, and
then falls back to `cargo run --bin cpl`.

The runner reports:

- `file@K`: expected file is present in the top K retrieved chunks;
- `symbol@K`: expected symbol is present in top K symbols or previews;
- confidence threshold pass/fail for each case.

Optional JSON output:

```powershell
python scripts/eval_retrieval.py --json-out .cpl\eval-results\retrieval.json
```

## CLI benchmark

The benchmark runner measures wall-clock latency for `scan`, `skeleton`, and
`retrieve` against the fixtures plus an optional extra root.

```powershell
cargo build --bins
python scripts/bench_cli.py --cpl .\target\debug\cpl.exe --iterations 3
```

Optional JSON output:

```powershell
python scripts/bench_cli.py --json-out .cpl\eval-results\bench.json
```

## MCP benchmark

The MCP benchmark measures the agent-facing warm path: one long-lived
`cpl-mcp` stdio process per root, with repeated `tools/call` requests against
the same in-memory `CognitiveProjectLayer`.

```powershell
cargo build --release --bins
python scripts/bench_mcp.py --mcp .\target\release\cpl-mcp.exe --iterations 5 --warmup 1
```

The output separates:

- `initialize`: MCP JSON-RPC handshake;
- `tools/list`: MCP tool discovery;
- `layer_init`: first `cpl_skeleton` call that builds the in-memory layer;
- `cpl_scan`, `cpl_skeleton`, `cpl_retrieve`: warm repeated MCP tool calls.

These scripts are intentionally stdlib-only so they can run in CI or local
developer environments without installing Python packages.

## Large synthetic benchmark

The large benchmark generates a temporary Rust repository and measures:

- cold scan;
- cold skeleton before a SQLite index exists;
- `index-build`;
- `index-freshness`;
- `index-refresh` when unchanged;
- `index-refresh` after one source file changed;
- `index-search` against the SQLite FTS index;
- warm skeleton after `.cpl/index.sqlite` exists;
- warm retrieval after `.cpl/index.sqlite` exists.
- local-hash `embed-index`;
- unchanged `embed-refresh`;
- lazy SQLite `embed-search`;
- one-file `embed-refresh`.

```powershell
cargo build --release --bins
python scripts/bench_large_repo.py --cpl .\target\release\cpl.exe --files 1000 --symbols-per-file 3 --iterations 3 --warmup 1
```

Optional JSON output:

```powershell
python scripts/bench_large_repo.py --json-out .cpl\eval-results\large-bench.json
```

Regression gate:

```powershell
python scripts/check_bench_thresholds.py --input .cpl\eval-results\large-bench.json
```

The gate checks p95 latency for cold scan, cold skeleton, structural index
build/freshness/refresh/search, warm skeleton/retrieval, and local-hash
embedding build/refresh/search. Thresholds can be overridden with repeated
`--threshold operation:milliseconds` arguments.

## GitHub Actions

Regular CI runs a smoke benchmark with one measured iteration to catch broken
commands quickly.

The dedicated `Benchmarks` workflow runs release binaries, writes a GitHub
Actions summary, and uploads JSON artifacts:

- manual: `workflow_dispatch`;
- scheduled: weekly on Monday;
- automatic: pushes to `main` that touch CPL source, evals, benchmark scripts,
  Cargo files, or the benchmark workflow itself.

Both CI and the scheduled benchmark workflow run `check_bench_thresholds.py` so
major latency regressions fail visibly instead of only being uploaded as data.
