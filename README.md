# Cognitive Project Layer

[![CI](https://github.com/kharkilirov1/cognitive-project-layer/actions/workflows/ci.yml/badge.svg)](https://github.com/kharkilirov1/cognitive-project-layer/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](Cargo.toml)

Local-first context engine for coding agents.

Cognitive Project Layer (CPL) gives an agent a structured, inspectable view of a
codebase: project skeleton, symbols, references, graph relations, local retrieval,
confidence scoring, and fallback tools exposed through CLI, MCP stdio, and an
optional local HTTP API.

The goal is not "RAG instead of understanding". The goal is a predictable context
pipeline for agents:

```text
scan -> skeleton -> symbols/references -> grep -> vector search -> graph expansion
     -> confidence -> managed context -> fallback plan
```

## Features

- Fast project scanner with ignored build/cache/vendor folders.
- Root `.gitignore` and `.cplignore` support for large/generated trees.
- Always-on `Skeleton` prompt: entry points, modules, configs, public API, recent changes.
- Tree-sitter-first symbol index for Rust, TypeScript/TSX, JavaScript, Python, C++, Go.
- Regex fallback for niche/unsupported syntax, including ArkTS/HarmonyOS components.
- References/usages index.
- Code-aware chunks with stable IDs and inferred line ranges.
- Local TF-IDF vector search with no network dependency.
- Persistent structural SQLite index in `.cpl/index.sqlite`.
- Warm-start from fresh SQLite structural indexes.
- Incremental SQLite index refresh with rebuild fallback.
- Index freshness diagnostics, MCP auto-refresh, and `cpl doctor`.
- Persistent embedding DB in `.cpl/vectors.sqlite` with legacy `.cpl/vector_db.json` fallback.
- Lazy SQLite vector loading with DB-backed streaming `embed-search`.
- Incremental embedding refresh by changed chunk path.
- Embedding backends:
  - `local-hash` offline default;
  - Ollama local neural embeddings;
  - OpenAI via `OPENAI_API_KEY`;
  - OpenAI-compatible endpoints.
- Qdrant external vector backend adapter.
- Structural graph: files, modules, configs, imports, call-ish references, tests.
- Confidence engine with `RagOnly`, `Hybrid`, and `FallbackExplore` strategies.
- Context budget manager and text transparency panel.
- CLI `cpl`, MCP stdio server `cpl-mcp`, Python MCP wrapper, and local HTTP API.
- Built-in local dashboard UI for health, retrieval, refresh actions, and eval history.
- File watcher and background refresh worker.
- Fixture-based retrieval evals and CLI latency benchmarks.
- Optional ArkTS/HarmonyOS profile; see [`docs/PROFILES.md`](docs/PROFILES.md).

## Quick install

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/kharkilirov1/cognitive-project-layer/main/install.ps1 | iex
```

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/kharkilirov1/cognitive-project-layer/main/install.sh | sh
```

Verify:

```powershell
cpl --version
cpl scan --root .
```

More options: [`docs/INSTALL.md`](docs/INSTALL.md).

## Install prebuilt binaries

Download the latest archive for your OS from
[`releases/latest`](https://github.com/kharkilirov1/cognitive-project-layer/releases/latest):

- `linux-x86_64`
- `windows-x86_64`
- `macos-x86_64`
- `macos-aarch64`

Each release also includes `SHA256SUMS`. The archives contain:

- `cpl`
- `cpl-mcp`
- `README.md`
- `LICENSE`
- `NOTICE`
- `CHANGELOG.md`
- `docs/`
- `install.sh`
- `install.ps1`

Put `cpl` and `cpl-mcp` on your `PATH`, or run them from the extracted folder.

## Install from source

Requirements:

- Rust 1.85+.
- Optional: Ollama if you want local neural embeddings.

```powershell
git clone https://github.com/kharkilirov1/cognitive-project-layer.git
cd cognitive-project-layer
cargo build --bins
```

Install directly with Cargo:

```powershell
cargo install --git https://github.com/kharkilirov1/cognitive-project-layer.git
```

The binaries are:

- `target/debug/cpl`
- `target/debug/cpl-mcp`

## Quick start

```powershell
cargo run -- scan --root .
cargo run -- skeleton --root .
cargo run -- symbols --root . retrieve
cargo run -- retrieve --root . "Where is retrieve implemented?"
cargo run -- context --root . --max-tokens 64000 "Why does the build fail around hilog?"
cargo run -- panel --root . "architecture retrieval"
cargo run -- index-build --root .
cargo run -- index-refresh --root .
cargo run -- index-db --root .
cargo run -- doctor --root .
```

After `cargo install --git`, use the installed binary:

```powershell
cpl scan --root .
cpl retrieve --root . "Where is retrieve implemented?"
cpl init --root . --server native
```

Build local embeddings:

```powershell
cargo run -- embed-index --root . --backend local-hash --dimensions 1536
cargo run -- embed-refresh --root . --backend local-hash --dimensions 1536
cargo run -- vector-db --root .
cargo run -- embed-search --root . "project graph retrieval" --limit 10
```

Build the structural SQLite index:

```powershell
cargo run -- index-build --root .
cargo run -- index-refresh --root .
cargo run -- index-search --root . "validate token"
cargo run -- index-db --root .
```

Persistence details: [`docs/PERSISTENCE.md`](docs/PERSISTENCE.md).

Use Ollama embeddings:

```powershell
ollama pull nomic-embed-text
cargo run -- embed-index --root . --backend ollama --model nomic-embed-text --dimensions 768
```

Use OpenAI embeddings:

```powershell
$env:OPENAI_API_KEY="..."
cargo run -- embed-index --root . --backend openai --model text-embedding-3-small
```

## MCP / OpenCode

Generate an OpenCode MCP config for a project:

```powershell
cargo run -- init --root . --server native
```

This writes a local `opencode.json` and ensures local/private state is ignored:

- `/.cpl`
- `.env`
- `.env.*`
- `opencode.json`

Portable examples are in `examples/opencode.native.json` and
`examples/opencode.python.json`.

If native stdio is problematic in a client, use the Python wrapper:

```powershell
cargo run -- init --root . --server python --force
```

Details and examples: [`docs/OPENCODE.md`](docs/OPENCODE.md).

## HTTP API

```powershell
cargo run -- serve --root . --host 127.0.0.1 --port 3878
```

Endpoints:

```text
GET  http://127.0.0.1:3878/health
GET  http://127.0.0.1:3878/ui
GET  http://127.0.0.1:3878/scan
GET  http://127.0.0.1:3878/skeleton
GET  http://127.0.0.1:3878/retrieve?query=symbol_lookup
GET  http://127.0.0.1:3878/context?query=auth%20login&max_tokens=64000
GET  http://127.0.0.1:3878/symbols?query=retrieve
GET  http://127.0.0.1:3878/references?symbol=retrieve
GET  http://127.0.0.1:3878/embed-search?query=opencode%20mcp&limit=5
POST http://127.0.0.1:3878/embeddings/rebuild
POST http://127.0.0.1:3878/embeddings/refresh
POST http://127.0.0.1:3878/index/rebuild
POST http://127.0.0.1:3878/index/refresh
GET  http://127.0.0.1:3878/index-db
GET  http://127.0.0.1:3878/index/freshness
GET  http://127.0.0.1:3878/index/search?query=validate%20token
GET  http://127.0.0.1:3878/doctor
GET  http://127.0.0.1:3878/benchmarks
GET  http://127.0.0.1:3878/tree?depth=3
GET  http://127.0.0.1:3878/grep?pattern=EntryAbility
```

Security note: keep the HTTP API bound to `127.0.0.1` unless you intentionally
want another process or machine to access your project context.

Dashboard details: [`docs/UI.md`](docs/UI.md).

## CLI overview

```text
cpl scan
cpl skeleton
cpl symbols [query]
cpl retrieve <query...>
cpl context [--max-tokens N] <query...>
cpl index
cpl index-build
cpl index-db
cpl index-freshness
cpl index-search <query...>
cpl index-refresh [--max-incremental-files N]
cpl doctor
cpl graph
cpl chunks [query]
cpl embed-index
cpl embed-refresh [--max-incremental-paths N]
cpl embed-search <query...>
cpl vector-db
cpl qdrant-upsert
cpl qdrant-search <query...>
cpl references <symbol>
cpl panel [query...]
cpl nav <section> [filter]
cpl git-status
cpl git-diff [range]
cpl tree --depth 3
cpl grep <pattern>
cpl watch
cpl serve
cpl init
```

## Architecture

```text
User query
  -> Query analyzer
  -> Skeleton always included
  -> Symbol lookup
  -> References/usages index
  -> Lexical search / grep
  -> Code-aware vector search
  -> Persistent embedding DB search
  -> Structural graph expansion
  -> Merge ranking
  -> Confidence engine
  -> Context budget manager
  -> Agent context / fallback plan / transparency panel
  -> Working memory update
```

## Repository layout

```text
src/
  lib.rs             CognitiveProjectLayer facade
  main.rs            CLI
  scanner.rs         project scan, mode selection, entry/config detection
  skeleton.rs        skeleton data model and prompt renderer
  symbols.rs         Tree-sitter-first symbol index + regex fallback
  ast.rs             Tree-sitter parsing helpers
  references.rs      references/usages index
  graph.rs           structural project graph
  retrieval.rs       hybrid retrieval pipeline
  confidence.rs      confidence engine
  doctor.rs          local diagnostics
  tools.rs           fallback tools
  memory.rs          working memory
  budget.rs          context budget manager
  chunk.rs           rich code-aware chunks
  vector.rs          local TF-IDF vector store
  embedding.rs       embedding providers
  persistent_index.rs SQLite structural index
  persistent_vector.rs SQLite persistent vector DB
  qdrant.rs          Qdrant adapter
  dashboard.rs       embedded local dashboard UI and benchmark summaries
  mcp_server.rs      MCP stdio server
  http_server.rs     local HTTP API
  watcher.rs         file watcher daemon
  background.rs      background refresh worker
scripts/
  cpl_opencode_mcp.py Python MCP wrapper
  eval_retrieval.py    fixture retrieval eval runner
  bench_cli.py         CLI latency benchmark runner
  bench_mcp.py         MCP stdio warm benchmark runner
  bench_large_repo.py  synthetic large-repository benchmark
  check_bench_thresholds.py benchmark regression gate
evals/
  retrieval.json       retrieval eval cases
  fixtures/            small Rust, TypeScript, and ArkTS projects
docs/
  INSTALL.md
  PERSISTENCE.md
  SCALE.md
  UI.md
  EVALS.md
  PROFILES.md
  OPENCODE.md
  SPEC.md
  ROADMAP.md
```

## Development

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --bins
```

Retrieval evals and CLI benchmarks:

```powershell
python scripts/eval_retrieval.py --cpl .\target\debug\cpl.exe --top-k 3
python scripts/bench_cli.py --cpl .\target\debug\cpl.exe --iterations 3
python scripts/bench_mcp.py --mcp .\target\debug\cpl-mcp.exe --iterations 3
```

Details: [`docs/EVALS.md`](docs/EVALS.md).

Do not commit local project cognition state:

- `.cpl/`
- `target/`
- `.env*`
- logs and caches

## License

Apache-2.0. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
