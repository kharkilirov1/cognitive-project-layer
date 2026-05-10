# Project config

CPL can read optional project settings from:

```text
.cpl/config.toml
```

Example:

```toml
[ignore]
paths = ["generated/", "vendor/", "*.snap"]

[embedding]
backend = "ollama"
model = "nomic-embed-text"
endpoint = "http://localhost:11434/v1/embeddings"
dimensions = 768

[context]
max_tokens = 64000

[benchmarks]
recall10 = "0.90"
ndcg10 = "0.70"

[ui]
default_tab = "graph"
```

Notes:

- Environment variables still override embedding config:
  `CPL_EMBEDDING_BACKEND`, `CPL_EMBEDDING_MODEL`,
  `CPL_EMBEDDING_ENDPOINT`, `CPL_EMBEDDING_DIMENSIONS`.
- `ignore.paths` augments `.gitignore` and `.cplignore`.
- `context.max_tokens` is used by `cpl serve` when `--max-tokens` is not
  explicitly changed from the default.
- A template is available at `examples/cpl.config.toml`.

## Self-heal and auto-refresh

Local self-heal is available through:

```powershell
cpl heal --root .
cpl doctor --root . --fix
```

Default behavior is conservative:

- refresh/rebuild `.cpl/index.sqlite`;
- refresh `.cpl/vectors.sqlite` only if it already exists;
- do not create missing embeddings unless `--embeddings ensure` is used.
- skip potentially external embedding backends unless an embedding backend is
  explicitly passed on the command line/tool call.

Useful modes:

```powershell
cpl heal --embeddings off
cpl heal --embeddings existing
cpl heal --embeddings ensure --embedding-backend local-hash --embedding-dimensions 1536
```

Runtime environment flags:

- `CPL_SELF_HEAL=0` disables HTTP server startup index self-heal.
- `CPL_INDEX_AUTO_REFRESH=0` disables MCP per-call index auto-refresh.
- `CPL_INDEX_REFRESH_LIMIT=128` controls incremental refresh fallback size.
- `CPL_INDEX_AUTO_REFRESH_INTERVAL_MS=2000` controls MCP refresh cadence.

`cpl serve` self-heals the persistent SQLite index on startup by default.
`cpl watch` also persists index refreshes after file-change batches.
