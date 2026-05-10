#!/usr/bin/env python3
"""Run a small public CodeSearchNet retrieval eval through the CPL CLI.

The runner downloads the MTEB CodeSearchNetRetrieval parquet files from
Hugging Face, materializes a temporary code corpus as one file per relevant
document, runs `cpl retrieve` for each natural-language query, and reports
standard retrieval metrics.

This is intentionally a quality benchmark, not a hardware-independent latency
benchmark. Wall-clock timings still depend on the local machine.
"""

from __future__ import annotations

import argparse
import json
import math
import shutil
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path


HF_BASE = "https://huggingface.co/datasets/mteb/CodeSearchNetRetrieval/resolve/main"
DEFAULT_CACHE = Path(".cpl/public-bench/codesearchnet")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--language", default="python", choices=["python", "go", "java", "javascript", "php", "ruby"])
    parser.add_argument("--limit", type=int, default=100, help="Number of qrels/queries to evaluate")
    parser.add_argument("--cpl", default=None, help="Path to cpl binary. Defaults to local release/debug binaries.")
    parser.add_argument("--cache-dir", default=str(DEFAULT_CACHE))
    parser.add_argument("--json-out", default=".cpl/eval-results/public-codesearchnet.json")
    parser.add_argument("--mode", choices=["cli", "http"], default="cli", help="CLI per-query mode or warm local HTTP mode")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=3891)
    parser.add_argument("--min-recall10", type=float, default=None, help="Fail if Recall@10 is below this value")
    parser.add_argument("--min-ndcg10", type=float, default=None, help="Fail if NDCG@10 is below this value")
    parser.add_argument("--keep", action="store_true", help="Keep the generated temporary corpus")
    return parser.parse_args()


def require_pandas():
    try:
        import pandas as pd  # type: ignore

        return pd
    except Exception as exc:
        raise SystemExit(
            "eval_public_codesearchnet.py requires pandas + pyarrow. "
            "Install them or run in the project dev environment."
        ) from exc


def resolve_cpl(explicit: str | None) -> str:
    if explicit:
        return explicit
    candidates = [
        Path(".cpl/verify-target/release/cpl.exe"),
        Path(".cpl/verify-target/debug/cpl.exe"),
        Path("target/release/cpl.exe"),
        Path("target/debug/cpl.exe"),
        Path("target/release/cpl"),
        Path("target/debug/cpl"),
    ]
    for candidate in candidates:
        if candidate.exists():
            return str(candidate)
    found = shutil.which("cpl")
    if found:
        return found
    raise SystemExit("cpl binary not found; run `cargo build --release --bins` or pass --cpl")


def download(cache: Path, language: str) -> tuple[Path, Path, Path]:
    cache.mkdir(parents=True, exist_ok=True)
    files = {
        "corpus": f"{language}-corpus/test-00000-of-00001.parquet",
        "queries": f"{language}-queries/test-00000-of-00001.parquet",
        "qrels": f"{language}-qrels/test-00000-of-00001.parquet",
    }
    paths = {}
    for key, remote in files.items():
        local = cache / remote.replace("/", "_")
        if not local.exists():
            print(f"download {remote}", file=sys.stderr)
            urllib.request.urlretrieve(f"{HF_BASE}/{remote}", local)
        paths[key] = local
    return paths["corpus"], paths["queries"], paths["qrels"]


def materialize_repo(pd, corpus_path: Path, queries_path: Path, qrels_path: Path, language: str, limit: int, cache: Path) -> tuple[Path, list[dict]]:
    corpus = pd.read_parquet(corpus_path)
    queries = pd.read_parquet(queries_path)
    qrels = pd.read_parquet(qrels_path).head(limit)

    root = cache / f"{language}-repo-{limit}"
    if root.exists():
        shutil.rmtree(root)
    source_dir = root / "src"
    source_dir.mkdir(parents=True)
    (root / ".gitignore").write_text(".cpl/\n", encoding="utf-8")
    (root / "Cargo.toml").write_text(
        '[package]\nname="codesearchnet_public_subset"\nversion="0.1.0"\nedition="2024"\n',
        encoding="utf-8",
    )

    corpus_by_id = corpus.set_index("id")
    queries_by_id = queries.set_index("id")
    manifest = []
    extension = {"python": "py", "javascript": "js", "java": "java", "go": "go", "php": "php", "ruby": "rb"}[language]
    for _, row in qrels.iterrows():
        query_id = str(row["query-id"])
        corpus_id = row["corpus-id"]
        code = corpus_by_id.loc[corpus_id]["text"]
        filename = f"case_{int(query_id):04d}.{extension}" if query_id.isdigit() else f"case_{len(manifest):04d}.{extension}"
        rel_path = f"src/{filename}"
        (source_dir / filename).write_text(f"# public CodeSearchNet {language} retrieval case\n\n{code}\n", encoding="utf-8")
        manifest.append(
            {
                "query_id": query_id,
                "corpus_id": corpus_id,
                "path": rel_path,
                "query": queries_by_id.loc[query_id]["text"],
            }
        )
    (cache / f"manifest-{language}-{limit}.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    return root, manifest


def rank_records_from_response(data: dict, item: dict) -> dict:
    chunks = data.get("chunks", []) or data.get("retrieval", {}).get("chunks", [])
    paths = [chunk.get("path", "").replace("\\", "/") for chunk in chunks]
    rank = next((idx for idx, path in enumerate(paths, 1) if path == item["path"]), None)
    return {
        "query_id": item["query_id"],
        "expected_path": item["path"],
        "rank": rank,
        "top_paths": paths[:10],
    }


def summarize_records(records: list[dict], elapsed: float) -> dict:
    total = len(records)

    def recall(k: int) -> float:
        return sum(1 for record in records if record["rank"] is not None and record["rank"] <= k) / total

    mrr = sum((1 / record["rank"]) if record["rank"] else 0.0 for record in records) / total
    ndcg10 = sum(
        (1 / math.log2(record["rank"] + 1)) if record["rank"] and record["rank"] <= 10 else 0.0
        for record in records
    ) / total
    return {
        "metrics": {
            "recall@1": recall(1),
            "recall@3": recall(3),
            "recall@5": recall(5),
            "recall@10": recall(10),
            "mrr": mrr,
            "ndcg@10": ndcg10,
            "elapsed_s": elapsed,
            "avg_query_s": elapsed / total,
        },
        "records": records,
    }


def run_eval_cli(cpl: str, root: Path, manifest: list[dict]) -> dict:
    subprocess.run([cpl, "--root", str(root), "index-build", "--json"], check=True, stdout=subprocess.DEVNULL)
    records = []
    start = time.perf_counter()
    for index, item in enumerate(manifest, 1):
        completed = subprocess.run(
            [cpl, "--root", str(root), "retrieve", item["query"], "--json"],
            check=True,
            capture_output=True,
            text=True,
        )
        data = json.loads(completed.stdout)
        records.append(rank_records_from_response(data, item))
        if index % 20 == 0:
            print(f"done {index}/{len(manifest)}", file=sys.stderr)
    return summarize_records(records, time.perf_counter() - start)


def wait_http_ready(host: str, port: int, timeout_s: float = 120.0) -> None:
    deadline = time.monotonic() + timeout_s
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            urllib.request.urlopen(f"http://{host}:{port}/health", timeout=1).read()
            return
        except Exception as exc:
            last_error = exc
            time.sleep(0.5)
    raise RuntimeError(f"CPL HTTP server did not become ready at {host}:{port}: {last_error}")


def run_eval_http(cpl: str, root: Path, manifest: list[dict], host: str, port: int) -> dict:
    subprocess.run([cpl, "--root", str(root), "index-build", "--json"], check=True, stdout=subprocess.DEVNULL)
    process = subprocess.Popen(
        [cpl, "--root", str(root), "serve", "--host", host, "--port", str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_http_ready(host, port)
        records = []
        start = time.perf_counter()
        for index, item in enumerate(manifest, 1):
            url = f"http://{host}:{port}/retrieve?query={urllib.parse.quote(item['query'])}"
            data = json.loads(urllib.request.urlopen(url, timeout=60).read().decode("utf-8"))
            records.append(rank_records_from_response(data, item))
            if index % 100 == 0 or index == len(manifest):
                print(f"done {index}/{len(manifest)}", file=sys.stderr)
        return summarize_records(records, time.perf_counter() - start)
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()


def main() -> int:
    args = parse_args()
    pd = require_pandas()
    cache = Path(args.cache_dir)
    corpus_path, queries_path, qrels_path = download(cache, args.language)
    root, manifest = materialize_repo(pd, corpus_path, queries_path, qrels_path, args.language, args.limit, cache)
    cpl = resolve_cpl(args.cpl)
    result = (
        run_eval_http(cpl, root, manifest, args.host, args.port)
        if args.mode == "http"
        else run_eval_cli(cpl, root, manifest)
    )
    result.update(
        {
            "benchmark": "mteb/CodeSearchNetRetrieval",
            "language": args.language,
            "mode": args.mode,
            "subset_size": len(manifest),
            "root": str(root),
            "source": "https://huggingface.co/datasets/mteb/CodeSearchNetRetrieval",
        }
    )
    output = Path(args.json_out)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result["metrics"], indent=2))
    print(f"saved {output}")
    failures = []
    if args.min_recall10 is not None and result["metrics"]["recall@10"] < args.min_recall10:
        failures.append(f"Recall@10 {result['metrics']['recall@10']:.4f} < {args.min_recall10:.4f}")
    if args.min_ndcg10 is not None and result["metrics"]["ndcg@10"] < args.min_ndcg10:
        failures.append(f"NDCG@10 {result['metrics']['ndcg@10']:.4f} < {args.min_ndcg10:.4f}")
    if not args.keep:
        shutil.rmtree(root, ignore_errors=True)
    if failures:
        print("Public CodeSearchNet quality gate failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
