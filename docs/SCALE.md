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
- Persistent embeddings exist in `.cpl/vector_db.json`.

Build or inspect the structural index:

```bash
cpl index-build --root .
cpl index-db --root .
```

HTTP/MCP equivalents:

```text
POST /index/rebuild
GET  /index-db
cpl_index_build
cpl_index_db
```

## Next scale milestone

The next major performance step is using the persisted index as the warm-start
source for large repositories:

- changed-file incremental refresh against the SQLite cache;
- warm startup from disk;
- large synthetic benchmark fixture and regression thresholds.
