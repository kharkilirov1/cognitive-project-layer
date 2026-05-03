# Changelog

All notable changes to this project will be documented in this file.

The format follows the spirit of Keep a Changelog, and this project uses
semantic versioning once stable releases begin.

## 0.7.0 - 2026-05-04

### Added

- Built-in local web dashboard served by `cpl serve` at `/ui`, `/dashboard`,
  and `/`.
- Dashboard panels for health, scan summary, doctor status, SQLite freshness,
  vector DB metadata, retrieval, FTS search, embedding search, maintenance
  refresh actions, and eval/benchmark history.
- HTTP endpoint `GET /benchmarks` for `.cpl/eval-results/*.json` summaries.
- `docs/UI.md` with dashboard usage and safety notes.

## 0.6.0 - 2026-05-04

### Added

- Lazy metadata-only loading for `.cpl/vectors.sqlite`.
- DB-backed streaming `embed-search` for SQLite vector DBs, avoiding eager
  in-process vector loads on warm agent startup.
- Large synthetic benchmark coverage for lazy SQLite embedding search.

### Changed

- `CognitiveProjectLayer::initialize()` now records persisted embedding counts
  without loading every dense vector into memory.
- Qdrant upsert explicitly eager-loads SQLite vector records before export.
- `cpl vector-db` summaries use SQLite metadata counts for lazy databases.

## 0.5.0 - 2026-05-04

### Added

- SQLite embedding store `.cpl/vectors.sqlite` with legacy `.cpl/vector_db.json`
  read fallback.
- Incremental embedding refresh through `cpl embed-refresh`, MCP
  `cpl_refresh_embeddings`, and HTTP `POST /embeddings/refresh`.
- SQLite FTS5 lexical chunk index inside `.cpl/index.sqlite`.
- FTS search through `cpl index-search`, MCP `cpl_index_search`, and HTTP
  `GET/POST /index/search`.
- Benchmark regression gate script `scripts/check_bench_thresholds.py`, wired
  into CI and the scheduled benchmark workflow.

### Changed

- `cpl embed-index` now writes `.cpl/vectors.sqlite` by default.
- Hybrid retrieval can use the persisted SQLite FTS index before grep fallback.
- Structural SQLite schema version increased to rebuild indexes with FTS.

## 0.4.0 - 2026-05-03

### Added

- Incremental structural SQLite refresh through `cpl index-refresh`.
- MCP tool `cpl_index_refresh` and HTTP endpoint `POST /index/refresh`.
- MCP auto-refresh before layer-backed tools, configurable with
  `CPL_INDEX_AUTO_REFRESH`, `CPL_INDEX_REFRESH_LIMIT`, and
  `CPL_INDEX_AUTO_REFRESH_INTERVAL_MS`.
- Large synthetic benchmark coverage for unchanged and one-file index refresh.

### Changed

- Layer-backed MCP calls reload the in-memory project layer after index refresh
  updates the SQLite cache.

## 0.3.0 - 2026-05-03

### Added

- Warm-start from fresh `.cpl/index.sqlite` snapshots for symbols, references, graph, and chunks.
- SQLite freshness diagnostics through `cpl index-freshness`, MCP, and HTTP.
- `cpl doctor` diagnostics for binaries, MCP config, SQLite index, vector DB, and Ollama.
- Large synthetic repository benchmark script and benchmark workflow artifact.

### Changed

- Python MCP wrapper now prefers installed/local CPL binaries and exposes SQLite/doctor tools.

## 0.2.0 - 2026-05-03

### Added

- Persistent structural SQLite index in `.cpl/index.sqlite`.
- CLI commands `cpl index-build` and `cpl index-db`.
- MCP tools `cpl_index_build` and `cpl_index_db`.
- HTTP endpoints `POST /index/rebuild` and `GET /index-db`.
- `docs/PERSISTENCE.md` covering structural and embedding persistence.

### Changed

- Release archives include the `docs/` directory.

## 0.1.3 - 2026-05-03

### Added

- Root `.gitignore` and `.cplignore` support for scanner, grep, tree, and watcher paths.
- Configurable context token budgets for CLI, MCP, and HTTP context flows.
- `docs/SCALE.md` with large-repository guidance and current scale limitations.

### Changed

- Default managed-context budget increased from `16_000` to `32_000` estimated tokens.

## 0.1.2 - 2026-05-03

### Added

- `install.ps1` for Windows PowerShell installs from GitHub Releases.
- `install.sh` for Linux/macOS installs from GitHub Releases.
- `docs/INSTALL.md` with quick install, custom install directory, and manual install.
- `cpl --version` and `cpl-mcp --version`.

### Changed

- Release archives now include install scripts.
- README now has a 30-second quick install path.

## 0.1.1 - 2026-05-03

### Added

- Fixture-based retrieval evals for Rust, TypeScript, and ArkTS/HarmonyOS.
- CLI benchmark runner with JSON output and GitHub Actions artifacts.
- MCP warm benchmark runner for the long-lived agent path.
- Dedicated scheduled benchmark workflow.
- Release workflow for multi-platform binary archives and SHA256 checksums.

### Changed

- Ignore `.playwright-mcp/` browser artifacts in scanner/tooling paths.
- Document eval and benchmark workflows.

## 0.1.0 - 2026-05-03

### Added

- Local project scanner and skeleton renderer.
- Tree-sitter-first symbol indexing with regex fallback.
- References/usages index.
- Code-aware chunks and local TF-IDF vector store.
- Persistent embedding DB with local-hash, Ollama, OpenAI, and OpenAI-compatible backends.
- Qdrant external vector backend adapter.
- Hybrid retrieval with confidence scoring and fallback plans.
- CLI, MCP stdio server, Python MCP wrapper, and local HTTP API.
- File watcher and background refresh worker.
- `cpl init` for OpenCode/MCP config generation.
- ArkTS/HarmonyOS support profile through ignores, entry/config detection, and parser fallback.
