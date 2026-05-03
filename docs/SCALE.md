# Scaling to larger repositories

CPL is designed to work best through a long-lived MCP/HTTP process. Cold CLI
calls rebuild project cognition from disk; warm MCP calls reuse the in-memory
layer.

## Ignore files

For large repositories, the first lever is excluding generated or irrelevant
trees. CPL now combines:

- built-in ignores: `target`, `.cpl`, `.git`, `node_modules`, build/cache dirs,
  HarmonyOS build dirs, browser artifacts, and common tool caches;
- root `.gitignore`;
- root `.cplignore`.

`.cplignore` supports simple, root-level patterns:

```gitignore
generated/
snapshots/
*.snap
fixtures/large-vendor/
```

Supported pattern behavior:

- blank lines and `# comments` are ignored;
- directory patterns ending in `/` ignore the whole tree;
- `*` wildcard is supported;
- leading `/` anchors the pattern at the project root;
- `!negation` patterns are currently ignored.

## Context token budget

The default managed-context budget is now `32_000` estimated tokens.

CLI:

```bash
cpl context --root . --max-tokens 64000 "where is auth handled?"
```

MCP:

```json
{
  "name": "cpl_context",
  "arguments": {
    "query": "where is auth handled?",
    "max_tokens": 64000
  }
}
```

HTTP:

```text
GET http://127.0.0.1:3878/context?query=auth&max_tokens=64000
```

Server-level defaults:

```bash
cpl serve --root . --max-tokens 64000
cpl-mcp --root . --max-tokens 64000
```

## Current scale profile

- Cold CLI path: best for diagnostics and scripts.
- Warm MCP/HTTP path: best for coding agents.
- `.gitignore` / `.cplignore`: primary control for monorepos and generated code.
- Persistent structural metadata exists in `.cpl/index.sqlite`.
- Fresh structural metadata can warm-start symbols, references, graph, and chunks.
- `index-refresh` incrementally updates changed files and falls back to full rebuild.
- SQLite FTS lexical search is persisted inside `.cpl/index.sqlite`.
- Persistent embeddings exist in `.cpl/vectors.sqlite` with legacy JSON fallback.
- `embed-refresh` updates vectors only for changed chunk paths when possible.

Build or inspect the structural index:

```bash
cpl index-build --root .
cpl index-refresh --root .
cpl index-search --root . "auth route"
cpl index-db --root .
cpl index-freshness --root .
cpl embed-refresh --root . --backend ollama --model nomic-embed-text --dimensions 768
cpl doctor --root .
```

HTTP/MCP equivalents:

```text
POST /index/rebuild
POST /index/refresh
GET  /index-db
GET  /index/freshness
GET  /index/search?query=...
POST /embeddings/refresh
cpl_index_build
cpl_index_db
cpl_index_freshness
cpl_index_search
cpl_index_refresh
cpl_refresh_embeddings
```

## Next scale milestone

Fresh SQLite indexes are now used as the warm-start source for symbols,
references, graph, chunks, lexical FTS, and embeddings. Changed-file refresh can
update structural and semantic SQLite caches without rebuilding the entire
project. The next scale milestone is deeper stress coverage and finer invalidation:

- stricter regression thresholds for the large synthetic benchmark;
- broader stress tests for symbol/reference invalidation across very large
  cross-file edits.
- optional DB-backed dense-vector search that avoids loading all vectors into
  process memory.
