# Roadmap

## 0.1.x — Public MVP hardening

Status: in progress.

- [x] CLI project scanner.
- [x] Ignored-aware tree/grep.
- [x] `.cpl/`, `target/`, caches, vendor/build folders excluded from scan.
- [x] Project skeleton prompt.
- [x] Tree-sitter-first symbol index.
- [x] Regex fallback for unsupported/niche syntax.
- [x] ArkTS/HarmonyOS support profile.
- [x] References/usages index.
- [x] Local TF-IDF vector store.
- [x] Persistent embedding DB.
- [x] Ollama/OpenAI/OpenAI-compatible/local-hash embedding backends.
- [x] Qdrant external vector adapter.
- [x] Structural project graph.
- [x] Hybrid retrieval pipeline.
- [x] Confidence engine and fallback plans.
- [x] Context budget manager.
- [x] Text transparency panel.
- [x] MCP stdio server and Python wrapper.
- [x] Local HTTP API.
- [x] File watcher and background refresh worker.
- [x] `cpl init` for OpenCode/MCP config generation.
- [x] Apache-2.0 license, contribution docs, security notes, CI.

## 0.2.x — Product quality

- [ ] Release binaries for Windows/Linux/macOS.
- [ ] Installer or `cargo install` flow documentation.
- [ ] Retrieval eval suite across public repositories.
- [ ] Golden tests for MCP tool outputs.
- [ ] Better error messages for embedding provider failures.
- [ ] Config file for profiles and ignored paths.
- [ ] Public demo repository and terminal recording.

## 0.3.x — Deeper code intelligence

- [ ] LSP-backed references/usages where available.
- [ ] Incremental symbol/chunk refresh for persistent embedding DB.
- [ ] More language profiles.
- [ ] Query-specific graph traversal policies.
- [ ] Optional embedded vector backend beyond JSON persistence.

## Later

- [ ] GUI transparency panel.
- [ ] Multi-repository workspace mode.
- [ ] Agent evaluation harness for edit success rate, not only retrieval quality.

