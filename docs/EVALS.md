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

These scripts are intentionally stdlib-only so they can run in CI or local
developer environments without installing Python packages.
