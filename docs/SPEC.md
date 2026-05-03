# Cognitive Project Layer Specification

## Goal

Cognitive Project Layer (CPL) is a local context layer between a repository and
a coding agent.

It should help an agent answer:

- What are the entry points?
- What modules and configs exist?
- What public APIs and symbols exist?
- Where is a symbol declared or used?
- Which files are structurally related?
- How confident is retrieval?
- When should the agent fall back to grep/tree/manual inspection?
- What context should always be present?

The core idea:

> Not RAG instead of project understanding, but RAG inside a structured project
> understanding system.

## Why plain RAG is not enough

Plain semantic retrieval often:

1. misses exact symbols;
2. loses project structure;
3. cannot explain confidence;
4. returns top-k even when matches are weak;
5. ignores recent changes and graph relations.

For coding agents, the preferred order is:

1. exact/fuzzy symbol lookup;
2. references/usages;
3. grep;
4. graph expansion;
5. vector search;
6. confidence scoring and fallback plan.

## Components

1. Project scanner
2. Mode selector
3. Skeleton
4. Symbol index
5. References index
6. Lazy indexer
7. Code-aware chunks
8. Local vector search
9. Persistent embedding DB
10. Structural graph
11. Confidence engine
12. Fallback tools
13. Working memory
14. Context budget manager
15. Transparency panel
16. CLI, MCP, HTTP interfaces

## Project scanner

The scanner collects:

- total files;
- source files;
- language counts;
- config files;
- entry candidates;
- ignored directories;
- recent git changes;
- complexity estimate;
- recommended context mode.

Ignored by default:

```text
node_modules/
target/
build/
dist/
.cpl/
.git/
.idea/
.vscode/
coverage/
.env
.env.local
.env.development
.env.production
.env.test
__pycache__/
.ruff_cache/
.ohos/
.hvigor/
hvigor/
entry/build/
oh_modules/
```

## Context modes

```rust
enum ContextMode {
    Full,
    Hybrid,
    Rag,
    Explorer,
}
```

Mode selection should consider:

- source file count;
- estimated tokens;
- language count;
- module depth;
- generated/build ratio.

## Skeleton

The skeleton is the always-on project map.

It contains:

- project metadata;
- entry points;
- modules;
- public API candidates;
- config files;
- recent changes;
- symbol summary.

The skeleton is stored as structured data and rendered into a prompt only at the
boundary.

## Symbol index

The symbol index answers:

- where a function/class/type is declared;
- what public APIs exist;
- what methods/types/components exist;
- where exported components are defined.

Current implementation:

- Tree-sitter-first parser for Rust, TypeScript/TSX, JavaScript, Python, C++, Go.
- Regex fallback for unsupported/niche syntax.
- ArkTS/HarmonyOS component/export fallback rules.
- Rust regex fallback is skipped when Tree-sitter successfully parsed symbols,
  preventing raw-string test fixtures from becoming real symbols.

## Lazy indexer

```rust
enum IndexState {
    Cold,
    Warming,
    Hot,
}
```

Touched, related, and warm files are tracked as sets inside `LazyIndexer`, not
as separate enum variants.

`Hot` must not block initial startup.

## Incremental refresh

On file save, CPL should refresh:

1. file cache;
2. scan metadata for that file;
3. symbols for that file;
4. references for that file;
5. graph surface for that file;
6. chunks for that file;
7. vector store chunks for that path;
8. persistent vector DB handle if present;
9. indexer status.

## Code-aware chunks

Chunks should preserve code structure:

```rust
struct RichChunk {
    path: PathBuf,
    line_start: usize,
    line_end: usize,
    source: String,
    signature: Option<String>,
    docs: Option<String>,
    chunk_type: ChunkType,
    symbols: Vec<String>,
    imports: Vec<String>,
    module_path: Vec<String>,
}
```

Embedding text should emphasize:

- signature;
- docs;
- symbol names;
- imports;
- module path;
- first source lines.

## Structural graph

Graph edges include:

- file imports file;
- file contains symbol;
- config affects file;
- call-ish references;
- test covers source;
- component uses component;
- native binding markers.

## Retrieval pipeline

1. Analyze query intent, terms, symbols, module hints.
2. Symbol lookup.
3. References/usages lookup.
4. Grep over ignored-aware text candidates.
5. Local vector search.
6. Persistent embedding DB search if available.
7. Skeleton context paths.
8. Graph expansion.
9. Working-memory boost.
10. Candidate scoring.
11. Confidence calculation.
12. Fallback plan generation.

## Confidence

Signals:

- top score;
- score gap;
- exact symbol match;
- module match;
- graph connection;
- recent-change match;
- result count;
- intent-specific profile.

Strategies:

```rust
enum RetrievalStrategy {
    RagOnly,
    Hybrid,
    FallbackExplore,
}
```

If confidence is low, CPL should explicitly tell the agent what fallback tools to
use instead of pretending that weak top-k results are enough.

## Interfaces

### CLI

Primary local interface: `cpl`.

### MCP

MCP tools:

- `cpl_scan`
- `cpl_skeleton`
- `cpl_retrieve`
- `cpl_context`
- `cpl_symbols`
- `cpl_references`
- `cpl_embed_search`
- `cpl_build_embeddings`
- `cpl_tree`
- `cpl_grep`
- `cpl_panel`

### HTTP

Optional local HTTP API for agents and scripts. Default bind target should remain
loopback (`127.0.0.1`) for safety.

## Profiles

Framework-specific rules should remain optional, isolated, and testable. See
`docs/PROFILES.md`.

Current public profile:

- ArkTS/HarmonyOS.

## Current limitations

- No LSP-backed semantic references yet.
- No GUI transparency panel yet; current panel is text-based.
- Persistent vector DB refresh is chunk-path incremental, but embedding model
  changes still require a full vector rebuild.
- SQLite vector search streams from the DB, but exact dense scoring is still
  linear over persisted vectors; approximate ANN is left to Qdrant or a future
  embedded ANN backend.
