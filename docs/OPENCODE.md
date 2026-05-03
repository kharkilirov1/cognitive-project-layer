# OpenCode integration

OpenCode can use Cognitive Project Layer as a local MCP server.

## Generate config

Native Rust MCP server:

```powershell
cargo build --bins
cargo run -- init --root . --server native
```

Python wrapper fallback:

```powershell
cargo run -- init --root . --server python --force
```

Use `--force` to overwrite an existing `opencode.json`.

`opencode.json` is intended to be local because generated configs often contain
machine-specific paths. Portable examples are stored under `examples/`.

## Generic native config

If `cpl-mcp` is on `PATH`, a portable config looks like this:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "cpl": {
      "type": "local",
      "command": ["cpl-mcp", "--root", "."],
      "enabled": true,
      "timeout": 300000,
      "environment": {
        "CPL_EMBEDDING_BACKEND": "ollama",
        "CPL_EMBEDDING_MODEL": "nomic-embed-text",
        "CPL_EMBEDDING_DIMENSIONS": "768",
        "CPL_INDEX_AUTO_REFRESH": "1",
        "CPL_INDEX_REFRESH_LIMIT": "128",
        "CPL_INDEX_AUTO_REFRESH_INTERVAL_MS": "2000"
      }
    }
  }
}
```

Example files:

- `examples/opencode.native.json`
- `examples/opencode.python.json`

## Available MCP tools

- `cpl_scan` — project scan.
- `cpl_skeleton` — always-on project skeleton.
- `cpl_retrieve` — hybrid retrieval for a coding-agent query.
- `cpl_context` — managed LLM context with token budget.
- `cpl_symbols` — exact/fuzzy symbol lookup.
- `cpl_references` — symbol usages/references.
- `cpl_index_build` — build `.cpl/index.sqlite`.
- `cpl_index_db` — inspect SQLite index summary.
- `cpl_index_freshness` — check whether SQLite index matches current files.
- `cpl_index_refresh` — incrementally refresh SQLite index or rebuild when needed.
- `cpl_embed_search` — search persistent local neural embedding DB.
- `cpl_build_embeddings` — rebuild persistent embeddings DB; defaults to Ollama `nomic-embed-text`.
- `cpl_tree` — ignored-aware project file tree.
- `cpl_grep` — grep over project text.
- `cpl_panel` — text transparency/status panel.

## Example prompts

```text
Use cpl_retrieve to find files related to symbol lookup, then inspect the code before editing.
```

```text
Use cpl_context for "why does retrieval miss references" and then propose a fix.
```

```text
Use cpl_embed_search for "local embedding ollama backend".
```

## Embeddings

Build/update local neural embeddings:

```powershell
ollama pull nomic-embed-text
cargo run -- embed-index --root . --backend ollama --model nomic-embed-text --dimensions 768
```

Generated DB:

```text
.cpl/vector_db.json
```

Do not commit `.cpl/vector_db.json` for private repositories. It contains data
derived from source code.

## Alternative HTTP API

```powershell
cargo run -- serve --root . --host 127.0.0.1 --port 3878
```

Then local agents can call:

```text
GET http://127.0.0.1:3878/retrieve?query=symbol_lookup
GET http://127.0.0.1:3878/embed-search?query=opencode%20mcp&limit=5
POST http://127.0.0.1:3878/embeddings/rebuild
```
