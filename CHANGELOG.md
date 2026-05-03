# Changelog

All notable changes to this project will be documented in this file.

The format follows the spirit of Keep a Changelog, and this project uses
semantic versioning once stable releases begin.

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
