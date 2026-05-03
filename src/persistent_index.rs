use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::chunk::RichChunk;
use crate::embedding::text_hash;
use crate::graph::ProjectGraph;
use crate::references::ReferenceIndex;
use crate::scanner::{ProjectScan, detect_language, is_config_file, is_entry_candidate};
use crate::symbols::SymbolIndex;

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

    fn temp_project(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cpl-{name}-{}-{nanos}", std::process::id()))
    }
}
