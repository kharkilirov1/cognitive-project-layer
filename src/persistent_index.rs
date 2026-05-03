use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::chunk::{ChunkType, RichChunk};
use crate::embedding::text_hash;
use crate::graph::{GraphEdge, GraphEdgeKind, GraphNode, GraphNodeKind, ProjectGraph};
use crate::references::{ReferenceIndex, ReferenceKind, ReferenceLocation};
use crate::scanner::{ProjectScan, detect_language, is_config_file, is_entry_candidate};
use crate::symbols::{SymbolIndex, SymbolKind, SymbolLocation, Visibility};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentIndexSummary {
    pub path: PathBuf,
    pub schema_version: u32,
    pub root: PathBuf,
    pub built_unix: u64,
    pub files: usize,
    pub symbols: usize,
    pub references: usize,
    pub chunks: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentIndexFreshness {
    pub path: PathBuf,
    pub exists: bool,
    pub fresh: bool,
    pub schema_version: Option<u32>,
    pub reason: String,
    pub current_files: usize,
    pub indexed_files: usize,
    pub changed_files: Vec<PersistentIndexFileChange>,
    pub missing_files: Vec<PathBuf>,
    pub extra_files: Vec<PathBuf>,
}

impl PersistentIndexFreshness {
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str("Persistent SQLite index freshness\n");
        out.push_str(&format!("Path: {}\n", self.path.display()));
        out.push_str(&format!("Exists: {}\n", self.exists));
        out.push_str(&format!("Fresh: {}\n", self.fresh));
        out.push_str(&format!(
            "Schema: {}\n",
            self.schema_version
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));
        out.push_str(&format!("Reason: {}\n", self.reason));
        out.push_str(&format!(
            "Files: current={} indexed={}\n",
            self.current_files, self.indexed_files
        ));
        append_change_paths(&mut out, "Changed files", &self.changed_files, 10);
        append_paths(&mut out, "Missing from index", &self.missing_files, 10);
        append_paths(&mut out, "Extra in index", &self.extra_files, 10);
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentIndexFileChange {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentIndexSnapshot {
    pub summary: PersistentIndexSummary,
    pub symbols: SymbolIndex,
    pub references: ReferenceIndex,
    pub graph: ProjectGraph,
    pub chunks: Vec<RichChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PersistentIndexRefreshMode {
    Unchanged,
    Incremental,
    Rebuilt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentIndexRefreshResult {
    pub mode: PersistentIndexRefreshMode,
    pub path: PathBuf,
    pub touched_files: usize,
    pub summary: PersistentIndexSummary,
    pub freshness_before: PersistentIndexFreshness,
    pub freshness_after: PersistentIndexFreshness,
}

impl PersistentIndexRefreshResult {
    pub fn render_human(&self) -> String {
        format!(
            "Persistent SQLite index refresh\nMode: {:?}\nTouched files: {}\nBefore: {}\nAfter: {}\n\n{}",
            self.mode,
            self.touched_files,
            self.freshness_before.reason,
            self.freshness_after.reason,
            self.summary.render_human()
        )
    }
}

impl PersistentIndexSummary {
    pub fn render_human(&self) -> String {
        format!(
            "Persistent SQLite index\nSchema: {}\nRoot: {}\nFiles: {}\nSymbols: {}\nReferences: {}\nChunks: {}\nGraph nodes: {}\nGraph edges: {}\nBuilt unix: {}\nPath: {}",
            self.schema_version,
            self.root.display(),
            self.files,
            self.symbols,
            self.references,
            self.chunks,
            self.graph_nodes,
            self.graph_edges,
            self.built_unix,
            self.path.display()
        )
    }
}

pub struct PersistentIndex;

impl PersistentIndex {
    pub fn default_path(root: &Path) -> PathBuf {
        root.join(".cpl").join("index.sqlite")
    }

    pub fn build_default(
        root: &Path,
        scan: &ProjectScan,
        symbols: &SymbolIndex,
        references: &ReferenceIndex,
        graph: &ProjectGraph,
        chunks: &[RichChunk],
    ) -> Result<(PersistentIndexSummary, PathBuf)> {
        let path = Self::default_path(root);
        let summary = Self::build(&path, root, scan, symbols, references, graph, chunks)?;
        Ok((summary, path))
    }

    pub fn build(
        path: &Path,
        root: &Path,
        scan: &ProjectScan,
        symbols: &SymbolIndex,
        references: &ReferenceIndex,
        graph: &ProjectGraph,
        chunks: &[RichChunk],
    ) -> Result<PersistentIndexSummary> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let mut conn = Connection::open(path)
            .with_context(|| format!("failed to open SQLite index {}", path.display()))?;
        create_schema(&conn)?;

        let built_unix = current_unix()?;
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let file_paths = indexed_file_paths(&root, scan);
        let source_paths = scan
            .source_paths
            .iter()
            .map(|path| relative_path(&root, path))
            .collect::<BTreeSet<_>>();
        let config_paths = scan.config_files.iter().cloned().collect::<BTreeSet<_>>();
        let entry_paths = scan
            .entry_candidates
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let tx = conn.transaction()?;
        clear_tables(&tx)?;
        write_meta(&tx, "schema_version", SCHEMA_VERSION.to_string())?;
        write_meta(&tx, "root", root.to_string_lossy())?;
        write_meta(&tx, "built_unix", built_unix.to_string())?;
        write_meta(&tx, "package_version", env!("CARGO_PKG_VERSION"))?;

        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO files \
                (path, language, size_bytes, modified_unix, sha256, is_source, is_config, is_entry) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for rel in &file_paths {
                let abs = root.join(rel);
                if !abs.is_file() {
                    continue;
                }
                let metadata = fs::metadata(&abs)
                    .with_context(|| format!("failed to stat {}", abs.display()))?;
                let sha256 = sha256_file(&abs)
                    .with_context(|| format!("failed to hash {}", abs.display()))?;
                let is_config = config_paths.contains(rel) || is_config_file(&abs);
                let is_entry = entry_paths.contains(rel) || is_entry_candidate(&abs, rel);
                stmt.execute(params![
                    path_text(rel),
                    detect_language(&abs),
                    metadata.len() as i64,
                    modified_unix(&metadata).map(|value| value as i64),
                    sha256,
                    bool_i64(source_paths.contains(rel)),
                    bool_i64(is_config),
                    bool_i64(is_entry),
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbols \
                (file_path, name, kind, line_start, line_end, signature, visibility) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for symbol in &symbols.symbols {
                let rel = relative_path(&root, &symbol.path);
                stmt.execute(params![
                    path_text(&rel),
                    symbol.name,
                    format!("{:?}", symbol.kind),
                    symbol.line_start as i64,
                    symbol.line_end as i64,
                    symbol.signature,
                    format!("{:?}", symbol.visibility),
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO references_idx \
                (file_path, symbol_name, line_number, column_start, kind, snippet) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for reference in references.by_symbol.values().flatten() {
                let rel = relative_path(&root, &reference.path);
                stmt.execute(params![
                    path_text(&rel),
                    reference.symbol_name,
                    reference.line_number as i64,
                    reference.column_start as i64,
                    format!("{:?}", reference.kind),
                    reference.snippet,
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO chunks \
                (id, file_path, line_start, line_end, chunk_type, signature, source, docs, \
                 symbols_json, imports_json, module_path_json, text_hash) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for chunk in chunks {
                stmt.execute(params![
                    chunk.id,
                    path_text(&chunk.path),
                    chunk.line_start as i64,
                    chunk.line_end as i64,
                    format!("{:?}", chunk.chunk_type),
                    chunk.signature,
                    chunk.source,
                    chunk.docs,
                    serde_json::to_string(&chunk.symbols)?,
                    serde_json::to_string(&chunk.imports)?,
                    serde_json::to_string(&chunk.module_path)?,
                    text_hash(&chunk.embed_text()),
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO graph_nodes (id, kind, label, path) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for node in &graph.nodes {
                stmt.execute(params![
                    node.id,
                    format!("{:?}", node.kind),
                    node.label,
                    node.path.as_ref().map(|path| path_text(path)),
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO graph_edges (from_id, to_id, kind, evidence) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for edge in &graph.edges {
                stmt.execute(params![
                    edge.from,
                    edge.to,
                    format!("{:?}", edge.kind),
                    edge.evidence,
                ])?;
            }
        }

        let summary = PersistentIndexSummary {
            path: path.to_path_buf(),
            schema_version: SCHEMA_VERSION,
            root: root.clone(),
            built_unix,
            files: count_table(&tx, "files")?,
            symbols: count_table(&tx, "symbols")?,
            references: count_table(&tx, "references_idx")?,
            chunks: count_table(&tx, "chunks")?,
            graph_nodes: count_table(&tx, "graph_nodes")?,
            graph_edges: count_table(&tx, "graph_edges")?,
        };
        write_summary_meta(&tx, &summary)?;
        tx.commit()?;

        Ok(summary)
    }

    pub fn summary_default(root: &Path) -> Result<Option<PersistentIndexSummary>> {
        let path = Self::default_path(root);
        if !path.exists() {
            return Ok(None);
        }
        Self::summary(&path).map(Some)
    }

    pub fn summary(path: &Path) -> Result<PersistentIndexSummary> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open SQLite index {}", path.display()))?;
        let schema_version = meta_value(&conn, "schema_version")?
            .unwrap_or_else(|| "0".to_string())
            .parse::<u32>()
            .unwrap_or_default();
        let root = meta_value(&conn, "root")?.unwrap_or_default();
        let built_unix = meta_value(&conn, "built_unix")?
            .unwrap_or_else(|| "0".to_string())
            .parse::<u64>()
            .unwrap_or_default();

        Ok(PersistentIndexSummary {
            path: path.to_path_buf(),
            schema_version,
            root: PathBuf::from(root),
            built_unix,
            files: count_table(&conn, "files")?,
            symbols: count_table(&conn, "symbols")?,
            references: count_table(&conn, "references_idx")?,
            chunks: count_table(&conn, "chunks")?,
            graph_nodes: count_table(&conn, "graph_nodes")?,
            graph_edges: count_table(&conn, "graph_edges")?,
        })
    }

    pub fn freshness_default(root: &Path, scan: &ProjectScan) -> Result<PersistentIndexFreshness> {
        Self::freshness(&Self::default_path(root), root, scan)
    }

    pub fn refresh_incremental_default(
        root: &Path,
        scan: &ProjectScan,
        max_incremental_files: usize,
    ) -> Result<Option<PersistentIndexRefreshResult>> {
        let path = Self::default_path(root);
        Self::refresh_incremental(&path, root, scan, max_incremental_files)
    }

    pub fn refresh_incremental(
        path: &Path,
        root: &Path,
        scan: &ProjectScan,
        max_incremental_files: usize,
    ) -> Result<Option<PersistentIndexRefreshResult>> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let freshness_before = Self::freshness(path, &root, scan)?;
        if !freshness_before.exists || freshness_before.schema_version != Some(SCHEMA_VERSION) {
            return Ok(None);
        }

        let touched = refresh_touched_paths(&freshness_before);
        if touched.is_empty() {
            let summary = Self::summary(path)?;
            return Ok(Some(PersistentIndexRefreshResult {
                mode: PersistentIndexRefreshMode::Unchanged,
                path: path.to_path_buf(),
                touched_files: 0,
                summary,
                freshness_after: freshness_before.clone(),
                freshness_before,
            }));
        }
        if touched.len() > max_incremental_files {
            return Ok(None);
        }

        let mut snapshot = Self::load_snapshot(path, &root)?;
        for rel in &touched {
            let abs = root.join(rel);
            snapshot.symbols.refresh_file(&abs)?;
            snapshot
                .references
                .refresh_file(&root, &abs, &snapshot.symbols)?;
            snapshot
                .graph
                .refresh_file(&root, scan, &snapshot.symbols, &abs)?;

            snapshot.chunks.retain(|chunk| chunk.path != *rel);
            if abs.exists() {
                snapshot
                    .chunks
                    .extend(crate::chunk::chunk_file(&root, &abs, &snapshot.symbols)?);
            }
        }
        snapshot.chunks.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.line_start.cmp(&right.line_start))
                .then_with(|| left.id.cmp(&right.id))
        });

        let summary = write_incremental_snapshot(path, &root, scan, &snapshot, &touched)?;
        let freshness_after = Self::freshness(path, &root, scan)?;
        Ok(Some(PersistentIndexRefreshResult {
            mode: PersistentIndexRefreshMode::Incremental,
            path: path.to_path_buf(),
            touched_files: touched.len(),
            summary,
            freshness_before,
            freshness_after,
        }))
    }

    pub fn freshness(
        path: &Path,
        root: &Path,
        scan: &ProjectScan,
    ) -> Result<PersistentIndexFreshness> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let current_files = current_indexed_files(&root, scan)?;
        if !path.exists() {
            return Ok(PersistentIndexFreshness {
                path: path.to_path_buf(),
                exists: false,
                fresh: false,
                schema_version: None,
                reason: "index file does not exist".to_string(),
                current_files: current_files.len(),
                indexed_files: 0,
                changed_files: Vec::new(),
                missing_files: current_files.keys().cloned().collect(),
                extra_files: Vec::new(),
            });
        }

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open SQLite index {}", path.display()))?;
        let schema_version =
            meta_value(&conn, "schema_version")?.and_then(|value| value.parse::<u32>().ok());
        if schema_version != Some(SCHEMA_VERSION) {
            return Ok(PersistentIndexFreshness {
                path: path.to_path_buf(),
                exists: true,
                fresh: false,
                schema_version,
                reason: format!(
                    "schema mismatch: expected {}, got {}",
                    SCHEMA_VERSION,
                    schema_version
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ),
                current_files: current_files.len(),
                indexed_files: count_table(&conn, "files").unwrap_or_default(),
                changed_files: Vec::new(),
                missing_files: Vec::new(),
                extra_files: Vec::new(),
            });
        }

        let indexed_files = load_indexed_file_metadata(&conn)?;
        let mut changed_files = Vec::new();
        let mut missing_files = Vec::new();
        let mut extra_files = Vec::new();

        for (rel, current) in &current_files {
            let Some(indexed) = indexed_files.get(rel) else {
                missing_files.push(rel.clone());
                continue;
            };
            if current.size_bytes != indexed.size_bytes {
                changed_files.push(PersistentIndexFileChange {
                    path: rel.clone(),
                    reason: format!(
                        "size changed: {} -> {}",
                        indexed.size_bytes, current.size_bytes
                    ),
                });
                continue;
            }
            if current.modified_unix != indexed.modified_unix {
                let sha256 = sha256_file(&root.join(rel))
                    .with_context(|| format!("failed to hash {}", root.join(rel).display()))?;
                if Some(sha256) != indexed.sha256 {
                    changed_files.push(PersistentIndexFileChange {
                        path: rel.clone(),
                        reason: "modified time and content hash changed".to_string(),
                    });
                }
            }
        }

        for rel in indexed_files.keys() {
            if !current_files.contains_key(rel) {
                extra_files.push(rel.clone());
            }
        }

        let fresh = changed_files.is_empty() && missing_files.is_empty() && extra_files.is_empty();
        let reason = if fresh {
            "index is fresh".to_string()
        } else {
            format!(
                "changed={}, missing={}, extra={}",
                changed_files.len(),
                missing_files.len(),
                extra_files.len()
            )
        };

        Ok(PersistentIndexFreshness {
            path: path.to_path_buf(),
            exists: true,
            fresh,
            schema_version,
            reason,
            current_files: current_files.len(),
            indexed_files: indexed_files.len(),
            changed_files,
            missing_files,
            extra_files,
        })
    }

    pub fn load_snapshot_default(
        root: &Path,
        scan: &ProjectScan,
    ) -> Result<Option<PersistentIndexSnapshot>> {
        let path = Self::default_path(root);
        if !path.exists() {
            return Ok(None);
        }
        let freshness = Self::freshness(&path, root, scan)?;
        if !freshness.fresh {
            return Ok(None);
        }
        Self::load_snapshot(&path, root).map(Some)
    }

    pub fn load_snapshot(path: &Path, root: &Path) -> Result<PersistentIndexSnapshot> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open SQLite index {}", path.display()))?;
        let summary = Self::summary(path)?;
        if summary.schema_version != SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported SQLite index schema {}; expected {}",
                summary.schema_version,
                SCHEMA_VERSION
            );
        }
        let symbols = load_symbols(&conn, &root)?;
        let references = load_references(&conn)?;
        let graph = load_graph(&conn)?;
        let chunks = load_chunks(&conn)?;
        Ok(PersistentIndexSnapshot {
            summary,
            symbols,
            references,
            graph,
            chunks,
        })
    }
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS files (
            path TEXT PRIMARY KEY,
            language TEXT,
            size_bytes INTEGER NOT NULL,
            modified_unix INTEGER,
            sha256 TEXT NOT NULL,
            is_source INTEGER NOT NULL,
            is_config INTEGER NOT NULL,
            is_entry INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_path TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            signature TEXT NOT NULL,
            visibility TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS references_idx (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_path TEXT NOT NULL,
            symbol_name TEXT NOT NULL,
            line_number INTEGER NOT NULL,
            column_start INTEGER NOT NULL,
            kind TEXT NOT NULL,
            snippet TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chunks (
            id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            chunk_type TEXT NOT NULL,
            signature TEXT,
            source TEXT NOT NULL,
            docs TEXT,
            symbols_json TEXT NOT NULL,
            imports_json TEXT NOT NULL,
            module_path_json TEXT NOT NULL,
            text_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS graph_nodes (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            label TEXT NOT NULL,
            path TEXT
        );
        CREATE TABLE IF NOT EXISTS graph_edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            evidence TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_files_sha256 ON files(sha256);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_symbols_file_path ON symbols(file_path);
        CREATE INDEX IF NOT EXISTS idx_references_symbol ON references_idx(symbol_name);
        CREATE INDEX IF NOT EXISTS idx_references_file_path ON references_idx(file_path);
        CREATE INDEX IF NOT EXISTS idx_chunks_file_path ON chunks(file_path);
        CREATE INDEX IF NOT EXISTS idx_graph_edges_from ON graph_edges(from_id);
        CREATE INDEX IF NOT EXISTS idx_graph_edges_to ON graph_edges(to_id);",
    )?;
    Ok(())
}

fn write_incremental_snapshot(
    path: &Path,
    root: &Path,
    scan: &ProjectScan,
    snapshot: &PersistentIndexSnapshot,
    touched: &BTreeSet<PathBuf>,
) -> Result<PersistentIndexSummary> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("failed to open SQLite index {}", path.display()))?;
    create_schema(&conn)?;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let source_paths = scan
        .source_paths
        .iter()
        .map(|path| relative_path(&root, path))
        .collect::<BTreeSet<_>>();
    let config_paths = scan.config_files.iter().cloned().collect::<BTreeSet<_>>();
    let entry_paths = scan
        .entry_candidates
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let built_unix = current_unix()?;
    let tx = conn.transaction()?;

    for rel in touched {
        let rel_text = path_text(rel);
        tx.execute("DELETE FROM files WHERE path = ?1", params![rel_text])?;
        tx.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            params![path_text(rel)],
        )?;
        tx.execute(
            "DELETE FROM references_idx WHERE file_path = ?1",
            params![path_text(rel)],
        )?;
        tx.execute(
            "DELETE FROM chunks WHERE file_path = ?1",
            params![path_text(rel)],
        )?;
    }

    for rel in touched {
        let abs = root.join(rel);
        if abs.is_file() {
            insert_file_row(&tx, &root, rel, &source_paths, &config_paths, &entry_paths)?;
        }
    }

    for symbol in &snapshot.symbols.symbols {
        let rel = relative_path(&root, &symbol.path);
        if touched.contains(&rel) {
            insert_symbol_row(&tx, &root, symbol)?;
        }
    }

    for reference in snapshot.references.by_symbol.values().flatten() {
        if touched.contains(&reference.path) {
            insert_reference_row(&tx, &root, reference)?;
        }
    }

    for chunk in &snapshot.chunks {
        if touched.contains(&chunk.path) {
            insert_chunk_row(&tx, chunk)?;
        }
    }

    tx.execute_batch("DELETE FROM graph_nodes; DELETE FROM graph_edges;")?;
    for node in &snapshot.graph.nodes {
        insert_graph_node_row(&tx, node)?;
    }
    for edge in &snapshot.graph.edges {
        insert_graph_edge_row(&tx, edge)?;
    }

    write_meta(&tx, "schema_version", SCHEMA_VERSION.to_string())?;
    write_meta(&tx, "root", root.to_string_lossy())?;
    write_meta(&tx, "built_unix", built_unix.to_string())?;
    write_meta(&tx, "package_version", env!("CARGO_PKG_VERSION"))?;

    let summary = PersistentIndexSummary {
        path: path.to_path_buf(),
        schema_version: SCHEMA_VERSION,
        root: root.clone(),
        built_unix,
        files: count_table(&tx, "files")?,
        symbols: count_table(&tx, "symbols")?,
        references: count_table(&tx, "references_idx")?,
        chunks: count_table(&tx, "chunks")?,
        graph_nodes: count_table(&tx, "graph_nodes")?,
        graph_edges: count_table(&tx, "graph_edges")?,
    };
    write_summary_meta(&tx, &summary)?;
    tx.commit()?;
    Ok(summary)
}

fn insert_file_row(
    conn: &Connection,
    root: &Path,
    rel: &Path,
    source_paths: &BTreeSet<PathBuf>,
    config_paths: &BTreeSet<PathBuf>,
    entry_paths: &BTreeSet<PathBuf>,
) -> Result<()> {
    let abs = root.join(rel);
    let metadata =
        fs::metadata(&abs).with_context(|| format!("failed to stat {}", abs.display()))?;
    let sha256 = sha256_file(&abs).with_context(|| format!("failed to hash {}", abs.display()))?;
    let is_config = config_paths.contains(rel) || is_config_file(&abs);
    let is_entry = entry_paths.contains(rel) || is_entry_candidate(&abs, rel);
    conn.execute(
        "INSERT OR REPLACE INTO files \
        (path, language, size_bytes, modified_unix, sha256, is_source, is_config, is_entry) \
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            path_text(rel),
            detect_language(&abs),
            metadata.len() as i64,
            modified_unix(&metadata).map(|value| value as i64),
            sha256,
            bool_i64(source_paths.contains(rel)),
            bool_i64(is_config),
            bool_i64(is_entry),
        ],
    )?;
    Ok(())
}

fn insert_symbol_row(conn: &Connection, root: &Path, symbol: &SymbolLocation) -> Result<()> {
    let rel = relative_path(root, &symbol.path);
    conn.execute(
        "INSERT INTO symbols \
        (file_path, name, kind, line_start, line_end, signature, visibility) \
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            path_text(&rel),
            symbol.name,
            format!("{:?}", symbol.kind),
            symbol.line_start as i64,
            symbol.line_end as i64,
            symbol.signature,
            format!("{:?}", symbol.visibility),
        ],
    )?;
    Ok(())
}

fn insert_reference_row(
    conn: &Connection,
    root: &Path,
    reference: &ReferenceLocation,
) -> Result<()> {
    let rel = relative_path(root, &reference.path);
    conn.execute(
        "INSERT INTO references_idx \
        (file_path, symbol_name, line_number, column_start, kind, snippet) \
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            path_text(&rel),
            reference.symbol_name,
            reference.line_number as i64,
            reference.column_start as i64,
            format!("{:?}", reference.kind),
            reference.snippet,
        ],
    )?;
    Ok(())
}

fn insert_chunk_row(conn: &Connection, chunk: &RichChunk) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO chunks \
        (id, file_path, line_start, line_end, chunk_type, signature, source, docs, \
         symbols_json, imports_json, module_path_json, text_hash) \
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            chunk.id,
            path_text(&chunk.path),
            chunk.line_start as i64,
            chunk.line_end as i64,
            format!("{:?}", chunk.chunk_type),
            chunk.signature,
            chunk.source,
            chunk.docs,
            serde_json::to_string(&chunk.symbols)?,
            serde_json::to_string(&chunk.imports)?,
            serde_json::to_string(&chunk.module_path)?,
            text_hash(&chunk.embed_text()),
        ],
    )?;
    Ok(())
}

fn insert_graph_node_row(conn: &Connection, node: &GraphNode) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO graph_nodes (id, kind, label, path) VALUES (?1, ?2, ?3, ?4)",
        params![
            node.id,
            format!("{:?}", node.kind),
            node.label,
            node.path.as_ref().map(|path| path_text(path)),
        ],
    )?;
    Ok(())
}

fn insert_graph_edge_row(conn: &Connection, edge: &GraphEdge) -> Result<()> {
    conn.execute(
        "INSERT INTO graph_edges (from_id, to_id, kind, evidence) VALUES (?1, ?2, ?3, ?4)",
        params![
            edge.from,
            edge.to,
            format!("{:?}", edge.kind),
            edge.evidence
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
struct FileMetadata {
    size_bytes: u64,
    modified_unix: Option<u64>,
    sha256: Option<String>,
}

fn current_indexed_files(
    root: &Path,
    scan: &ProjectScan,
) -> Result<BTreeMap<PathBuf, FileMetadata>> {
    let mut files = BTreeMap::new();
    for rel in indexed_file_paths(root, scan) {
        let abs = root.join(&rel);
        if !abs.is_file() {
            continue;
        }
        let metadata =
            fs::metadata(&abs).with_context(|| format!("failed to stat {}", abs.display()))?;
        files.insert(
            rel,
            FileMetadata {
                size_bytes: metadata.len(),
                modified_unix: modified_unix(&metadata),
                sha256: None,
            },
        );
    }
    Ok(files)
}

fn load_indexed_file_metadata(conn: &Connection) -> Result<BTreeMap<PathBuf, FileMetadata>> {
    let mut stmt =
        conn.prepare("SELECT path, size_bytes, modified_unix, sha256 FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let size_bytes: i64 = row.get(1)?;
        let modified_unix: Option<i64> = row.get(2)?;
        let sha256: String = row.get(3)?;
        Ok((
            PathBuf::from(path),
            FileMetadata {
                size_bytes: size_bytes as u64,
                modified_unix: modified_unix.map(|value| value as u64),
                sha256: Some(sha256),
            },
        ))
    })?;

    let mut out = BTreeMap::new();
    for row in rows {
        let (path, metadata) = row?;
        out.insert(path, metadata);
    }
    Ok(out)
}

fn load_symbols(conn: &Connection, root: &Path) -> Result<SymbolIndex> {
    let mut stmt = conn.prepare(
        "SELECT file_path, name, kind, line_start, line_end, signature, visibility
         FROM symbols
         ORDER BY file_path, line_start, name",
    )?;
    let rows = stmt.query_map([], |row| {
        let file_path: String = row.get(0)?;
        let kind: String = row.get(2)?;
        let visibility: String = row.get(6)?;
        Ok(SymbolLocation {
            path: root.join(file_path),
            name: row.get(1)?,
            kind: parse_symbol_kind(&kind),
            line_start: row.get::<_, i64>(3)? as usize,
            line_end: row.get::<_, i64>(4)? as usize,
            signature: row.get(5)?,
            visibility: parse_visibility(&visibility),
        })
    })?;

    let mut symbols = Vec::new();
    for row in rows {
        symbols.push(row?);
    }
    let by_name = symbol_map(&symbols);
    Ok(SymbolIndex { symbols, by_name })
}

fn load_references(conn: &Connection) -> Result<ReferenceIndex> {
    let mut stmt = conn.prepare(
        "SELECT file_path, symbol_name, line_number, column_start, kind, snippet
         FROM references_idx
         ORDER BY symbol_name, file_path, line_number, column_start",
    )?;
    let rows = stmt.query_map([], |row| {
        let kind: String = row.get(4)?;
        Ok(ReferenceLocation {
            path: PathBuf::from(row.get::<_, String>(0)?),
            symbol_name: row.get(1)?,
            line_number: row.get::<_, i64>(2)? as usize,
            column_start: row.get::<_, i64>(3)? as usize,
            kind: parse_reference_kind(&kind),
            snippet: row.get(5)?,
        })
    })?;

    let mut by_symbol = BTreeMap::<String, Vec<ReferenceLocation>>::new();
    for row in rows {
        let reference = row?;
        by_symbol
            .entry(reference.symbol_name.clone())
            .or_default()
            .push(reference);
    }
    Ok(ReferenceIndex { by_symbol })
}

fn load_graph(conn: &Connection) -> Result<ProjectGraph> {
    let mut node_stmt =
        conn.prepare("SELECT id, kind, label, path FROM graph_nodes ORDER BY id")?;
    let node_rows = node_stmt.query_map([], |row| {
        let kind: String = row.get(1)?;
        let path: Option<String> = row.get(3)?;
        Ok(GraphNode {
            id: row.get(0)?,
            kind: parse_graph_node_kind(&kind),
            label: row.get(2)?,
            path: path.map(PathBuf::from),
        })
    })?;
    let mut nodes = Vec::new();
    let mut file_nodes = BTreeMap::new();
    for row in node_rows {
        let node = row?;
        if node.kind == GraphNodeKind::File
            && let Some(path) = node.path.as_ref()
        {
            file_nodes.insert(path.clone(), node.id.clone());
        }
        nodes.push(node);
    }

    let mut edge_stmt =
        conn.prepare("SELECT from_id, to_id, kind, evidence FROM graph_edges ORDER BY id")?;
    let edge_rows = edge_stmt.query_map([], |row| {
        let kind: String = row.get(2)?;
        Ok(GraphEdge {
            from: row.get(0)?,
            to: row.get(1)?,
            kind: parse_graph_edge_kind(&kind),
            evidence: row.get(3)?,
        })
    })?;
    let mut edges = Vec::new();
    for row in edge_rows {
        edges.push(row?);
    }

    Ok(ProjectGraph {
        nodes,
        edges,
        file_nodes,
    })
}

fn load_chunks(conn: &Connection) -> Result<Vec<RichChunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, line_start, line_end, chunk_type, signature, source, docs,
                symbols_json, imports_json, module_path_json
         FROM chunks
         ORDER BY file_path, line_start, id",
    )?;
    let rows = stmt.query_map([], |row| {
        let chunk_type: String = row.get(4)?;
        let symbols_json: String = row.get(8)?;
        let imports_json: String = row.get(9)?;
        let module_path_json: String = row.get(10)?;
        Ok(RichChunk {
            id: row.get(0)?,
            path: PathBuf::from(row.get::<_, String>(1)?),
            line_start: row.get::<_, i64>(2)? as usize,
            line_end: row.get::<_, i64>(3)? as usize,
            chunk_type: parse_chunk_type(&chunk_type),
            signature: row.get(5)?,
            source: row.get(6)?,
            docs: row.get(7)?,
            symbols: serde_json::from_str(&symbols_json).unwrap_or_default(),
            imports: serde_json::from_str(&imports_json).unwrap_or_default(),
            module_path: serde_json::from_str(&module_path_json).unwrap_or_default(),
        })
    })?;
    let mut chunks = Vec::new();
    for row in rows {
        chunks.push(row?);
    }
    Ok(chunks)
}

fn clear_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM meta;
        DELETE FROM files;
        DELETE FROM symbols;
        DELETE FROM references_idx;
        DELETE FROM chunks;
        DELETE FROM graph_nodes;
        DELETE FROM graph_edges;",
    )?;
    Ok(())
}

fn write_summary_meta(conn: &Connection, summary: &PersistentIndexSummary) -> Result<()> {
    write_meta(conn, "files", summary.files.to_string())?;
    write_meta(conn, "symbols", summary.symbols.to_string())?;
    write_meta(conn, "references", summary.references.to_string())?;
    write_meta(conn, "chunks", summary.chunks.to_string())?;
    write_meta(conn, "graph_nodes", summary.graph_nodes.to_string())?;
    write_meta(conn, "graph_edges", summary.graph_edges.to_string())?;
    Ok(())
}

fn write_meta(conn: &Connection, key: &str, value: impl AsRef<str>) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![key, value.as_ref()],
    )?;
    Ok(())
}

fn meta_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn count_table(conn: &Connection, table: &str) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count = conn.query_row(&sql, [], |row| row.get::<_, i64>(0))?;
    Ok(count as usize)
}

fn indexed_file_paths(root: &Path, scan: &ProjectScan) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for path in &scan.source_paths {
        paths.insert(relative_path(root, path));
    }
    paths.extend(scan.config_files.iter().cloned());
    paths.extend(scan.entry_candidates.iter().cloned());
    paths
}

fn refresh_touched_paths(freshness: &PersistentIndexFreshness) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    paths.extend(
        freshness
            .changed_files
            .iter()
            .map(|change| change.path.clone()),
    );
    paths.extend(freshness.missing_files.iter().cloned());
    paths.extend(freshness.extra_files.iter().cloned());
    paths
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn bool_i64(value: bool) -> i64 {
    i64::from(value)
}

fn current_unix() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn modified_unix(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn symbol_map(symbols: &[SymbolLocation]) -> BTreeMap<String, Vec<usize>> {
    let mut by_name = BTreeMap::<String, Vec<usize>>::new();
    for (idx, symbol) in symbols.iter().enumerate() {
        by_name.entry(symbol.name.clone()).or_default().push(idx);
    }
    by_name
}

fn parse_symbol_kind(value: &str) -> SymbolKind {
    match value {
        "Function" => SymbolKind::Function,
        "Method" => SymbolKind::Method,
        "Class" => SymbolKind::Class,
        "Struct" => SymbolKind::Struct,
        "Enum" => SymbolKind::Enum,
        "Interface" => SymbolKind::Interface,
        "Component" => SymbolKind::Component,
        "Export" => SymbolKind::Export,
        "Const" => SymbolKind::Const,
        "TypeAlias" => SymbolKind::TypeAlias,
        "Trait" => SymbolKind::Trait,
        _ => SymbolKind::Unknown,
    }
}

fn parse_visibility(value: &str) -> Visibility {
    match value {
        "Public" => Visibility::Public,
        "Internal" => Visibility::Internal,
        _ => Visibility::Unknown,
    }
}

fn parse_reference_kind(value: &str) -> ReferenceKind {
    match value {
        "Call" => ReferenceKind::Call,
        "ComponentUse" => ReferenceKind::ComponentUse,
        _ => ReferenceKind::Identifier,
    }
}

fn parse_graph_node_kind(value: &str) -> GraphNodeKind {
    match value {
        "File" => GraphNodeKind::File,
        "Symbol" => GraphNodeKind::Symbol,
        "Module" => GraphNodeKind::Module,
        _ => GraphNodeKind::Config,
    }
}

fn parse_graph_edge_kind(value: &str) -> GraphEdgeKind {
    match value {
        "Imports" => GraphEdgeKind::Imports,
        "Exports" => GraphEdgeKind::Exports,
        "Calls" => GraphEdgeKind::Calls,
        "UsesComponent" => GraphEdgeKind::UsesComponent,
        "Tests" => GraphEdgeKind::Tests,
        "Configures" => GraphEdgeKind::Configures,
        "NativeBinding" => GraphEdgeKind::NativeBinding,
        "Contains" => GraphEdgeKind::Contains,
        _ => GraphEdgeKind::InModule,
    }
}

fn parse_chunk_type(value: &str) -> ChunkType {
    match value {
        "Function" => ChunkType::Function,
        "Class" => ChunkType::Class,
        "Struct" => ChunkType::Struct,
        "Enum" => ChunkType::Enum,
        "Interface" => ChunkType::Interface,
        "Component" => ChunkType::Component,
        "Config" => ChunkType::Config,
        "Test" => ChunkType::Test,
        "Route" => ChunkType::Route,
        "NativeBinding" => ChunkType::NativeBinding,
        "Module" => ChunkType::Module,
        _ => ChunkType::Unknown,
    }
}

fn append_change_paths(
    out: &mut String,
    label: &str,
    changes: &[PersistentIndexFileChange],
    limit: usize,
) {
    out.push_str(&format!("{label}:\n"));
    if changes.is_empty() {
        out.push_str("- none\n");
        return;
    }
    for change in changes.iter().take(limit) {
        out.push_str(&format!(
            "- {} ({})\n",
            change.path.display(),
            change.reason
        ));
    }
    if changes.len() > limit {
        out.push_str(&format!("- ... {} more\n", changes.len() - limit));
    }
}

fn append_paths(out: &mut String, label: &str, paths: &[PathBuf], limit: usize) {
    out.push_str(&format!("{label}:\n"));
    if paths.is_empty() {
        out.push_str("- none\n");
        return;
    }
    for path in paths.iter().take(limit) {
        out.push_str(&format!("- {}\n", path.display()));
    }
    if paths.len() > limit {
        out.push_str(&format!("- ... {} more\n", paths.len() - limit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CognitiveProjectLayer;

    #[test]
    fn builds_and_reads_sqlite_index_summary() {
        let root = temp_project("persistent_index_summary");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        fs::write(
            root.join("src").join("lib.rs"),
            "pub fn validate_token(token: &str) -> bool { !token.is_empty() }\n\
             pub fn login(token: &str) -> bool { validate_token(token) }\n",
        )
        .unwrap();

        let layer = CognitiveProjectLayer::initialize(&root).unwrap();
        let (summary, path) = PersistentIndex::build_default(
            &layer.root,
            &layer.scan,
            &layer.symbols,
            &layer.references,
            &layer.graph,
            &layer.vector_store.chunks,
        )
        .unwrap();

        assert!(path.exists());
        assert_eq!(summary.schema_version, SCHEMA_VERSION);
        assert!(summary.files >= 2);
        assert!(summary.symbols >= 2);
        assert!(summary.chunks >= 2);
        assert!(summary.graph_nodes >= 2);

        let loaded = PersistentIndex::summary_default(&layer.root)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.files, summary.files);
        assert_eq!(loaded.symbols, summary.symbols);
        assert_eq!(loaded.chunks, summary.chunks);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn freshness_detects_changed_file_and_snapshot_loads_when_fresh() {
        let root = temp_project("persistent_index_freshness");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        let file = root.join("src").join("lib.rs");
        fs::write(&file, "pub fn old_symbol() -> bool { true }\n").unwrap();

        let layer = CognitiveProjectLayer::initialize(&root).unwrap();
        PersistentIndex::build_default(
            &layer.root,
            &layer.scan,
            &layer.symbols,
            &layer.references,
            &layer.graph,
            &layer.vector_store.chunks,
        )
        .unwrap();

        let fresh = PersistentIndex::freshness_default(&layer.root, &layer.scan).unwrap();
        assert!(fresh.fresh, "{}", fresh.render_human());
        let snapshot = PersistentIndex::load_snapshot_default(&layer.root, &layer.scan)
            .unwrap()
            .unwrap();
        assert!(!snapshot.symbols.find("old_symbol").is_empty());

        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(&file, "pub fn new_symbol() -> bool { true }\n").unwrap();
        let changed_scan = crate::scanner::ProjectScanner::default()
            .scan(&root)
            .unwrap();
        let stale = PersistentIndex::freshness_default(&layer.root, &changed_scan).unwrap();
        assert!(!stale.fresh);
        assert!(
            stale
                .changed_files
                .iter()
                .any(|change| change.path == Path::new("src/lib.rs"))
        );
        assert!(
            PersistentIndex::load_snapshot_default(&layer.root, &changed_scan)
                .unwrap()
                .is_none()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_incremental_updates_changed_file_in_sqlite_index() {
        let root = temp_project("persistent_index_incremental_refresh");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        let file = root.join("src").join("lib.rs");
        fs::write(&file, "pub fn old_symbol() -> bool { true }\n").unwrap();

        let layer = CognitiveProjectLayer::initialize(&root).unwrap();
        PersistentIndex::build_default(
            &layer.root,
            &layer.scan,
            &layer.symbols,
            &layer.references,
            &layer.graph,
            &layer.vector_store.chunks,
        )
        .unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(&file, "pub fn new_symbol() -> bool { true }\n").unwrap();
        let changed_scan = crate::scanner::ProjectScanner::default()
            .scan(&root)
            .unwrap();
        let result = PersistentIndex::refresh_incremental_default(&root, &changed_scan, 128)
            .unwrap()
            .unwrap();

        assert_eq!(result.mode, PersistentIndexRefreshMode::Incremental);
        assert_eq!(result.touched_files, 1);
        assert!(result.freshness_after.fresh, "{}", result.render_human());

        let snapshot = PersistentIndex::load_snapshot_default(&root, &changed_scan)
            .unwrap()
            .unwrap();
        assert!(snapshot.symbols.find("old_symbol").is_empty());
        assert!(!snapshot.symbols.find("new_symbol").is_empty());

        let _ = fs::remove_dir_all(root);
    }

    fn temp_project(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cpl-{name}-{}-{nanos}", std::process::id()))
    }
}
