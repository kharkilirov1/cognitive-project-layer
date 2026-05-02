# Contributing

Thanks for helping improve Cognitive Project Layer.

## Development setup

Requirements:

- Rust 1.85+.
- Optional: Ollama with `nomic-embed-text` for local neural embeddings.

```powershell
cargo build --bins
cargo test
cargo clippy --all-targets -- -D warnings
```

## Project principles

- Keep the core local-first and vendor-neutral.
- Do not remove CLI/MCP/HTTP/fallback flows when adding integrations.
- Prefer exact navigation before semantic search: symbols, references, grep, graph, then embeddings.
- Keep language/framework-specific behavior behind profiles or clearly isolated modules.
- Do not commit generated local state: `.cpl/`, `target/`, `.env*`, logs, caches.

## Pull requests

Good PRs usually include:

1. A focused change.
2. Tests for behavior changes.
3. Documentation updates for user-facing changes.
4. A note about verification commands run locally.

## Security and secrets

Never commit tokens, API keys, private code embeddings, `.cpl/vector_db.json`, or local agent config containing private paths.

For vulnerability reports, see `SECURITY.md`.

