use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::chunk::{ChunkType, RichChunk};
use crate::embedding::{EmbeddingClient, EmbeddingConfig, cosine_dense, text_hash};

pub const VECTOR_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentVectorDb {
    pub version: u32,
    pub backend: String,
    pub model: String,
    pub dimensions: usize,
    pub root: PathBuf,
    pub created_unix: u64,
    pub records: Vec<VectorRecord>,
    #[serde(default)]
    pub storage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentVectorSummary {
    pub path: PathBuf,
    pub storage: String,
    pub version: u32,
    pub backend: String,
    pub model: String,
    pub dimensions: usize,
    pub root: PathBuf,
    pub created_unix: u64,
    pub records: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PersistentVectorRefreshMode {
    Unchanged,
    Incremental,
    Rebuilt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentVectorRefreshResult {
    pub mode: PersistentVectorRefreshMode,
    pub path: PathBuf,
    pub touched_paths: usize,
    pub records_before: usize,
    pub records_after: usize,
    pub summary: PersistentVectorSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRecord {
    pub id: String,
    pub chunk: RichChunk,
    pub vector: Vec<f32>,
    pub text_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentVectorHit {
    pub chunk: RichChunk,
    pub score: f32,
}

impl PersistentVectorSummary {
    pub fn render_human(&self) -> String {
        format!(
            "Persistent vector DB\nStorage: {}\nBackend: {}\nModel: {}\nDimensions: {}\nRecords: {}\nRoot: {}\nPath: {}",
            self.storage,
            self.backend,
            self.model,
            self.dimensions,
            self.records,
            self.root.display(),
            self.path.display()
        )
    }
}

impl PersistentVectorRefreshResult {
    pub fn render_human(&self) -> String {
        format!(
            "Persistent vector DB refresh\nMode: {:?}\nTouched paths: {}\nRecords: {} -> {}\n\n{}",
            self.mode,
            self.touched_paths,
            self.records_before,
            self.records_after,
            self.summary.render_human()
        )
    }
}

impl PersistentVectorDb {
    pub fn default_path(root: &Path) -> PathBuf {
        Self::sqlite_path(root)
    }

    pub fn sqlite_path(root: &Path) -> PathBuf {
        root.join(".cpl").join("vectors.sqlite")
    }

    pub fn json_path(root: &Path) -> PathBuf {
        root.join(".cpl").join("vector_db.json")
    }

    pub fn load_default(root: &Path) -> Result<Option<Self>> {
        let sqlite = Self::sqlite_path(root);
        if sqlite.exists() {
            return Self::load(&sqlite).map(Some);
        }
        let json = Self::json_path(root);
        if json.exists() {
            return Self::load(&json).map(Some);
        }
        Ok(None)
    }

    pub fn load(path: &Path) -> Result<Self> {
        if path.extension().and_then(|value| value.to_str()) == Some("sqlite") {
            Self::load_sqlite(path)
        } else {
            Self::load_json(path)
        }
    }

    pub fn load_json(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read vector db {}", path.display()))?;
        let mut db: Self = serde_json::from_str(&source)?;
        db.storage = "Json".to_string();
        db.path = Some(path.to_path_buf());
        Ok(db)
    }

    pub fn load_sqlite(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open vector DB {}", path.display()))?;
        create_schema(&conn)?;
        let version = meta_value(&conn, "schema_version")?
            .unwrap_or_else(|| "0".to_string())
            .parse::<u32>()
            .unwrap_or_default();
        if version != VECTOR_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported vector SQLite schema {}; expected {}",
                version,
                VECTOR_SCHEMA_VERSION
            );
        }
        let backend = meta_value(&conn, "backend")?.unwrap_or_else(|| "LocalHash".to_string());
        let model =
            meta_value(&conn, "model")?.unwrap_or_else(|| "local-hash-embedding".to_string());
        let dimensions = meta_value(&conn, "dimensions")?
            .unwrap_or_else(|| "1536".to_string())
            .parse::<usize>()
            .unwrap_or(1536);
        let root = PathBuf::from(meta_value(&conn, "root")?.unwrap_or_default());
        let created_unix = meta_value(&conn, "created_unix")?
            .unwrap_or_else(|| "0".to_string())
            .parse::<u64>()
            .unwrap_or_default();
        let records = load_records(&conn)?;
        Ok(Self {
            version,
            backend,
            model,
            dimensions,
            root,
            created_unix,
            records,
            storage: "SQLite".to_string(),
            path: Some(path.to_path_buf()),
        })
    }

    pub fn save_default(&self, root: &Path) -> Result<PathBuf> {
        let path = Self::sqlite_path(root);
        self.save(&path)?;
        Ok(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if path.extension().and_then(|value| value.to_str()) == Some("sqlite") {
            self.save_sqlite(path)
        } else {
            self.save_json(path)
        }
    }

    pub fn save_json(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn save_sqlite(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)
            .with_context(|| format!("failed to open vector DB {}", path.display()))?;
        create_schema(&conn)?;
        let tx = conn.transaction()?;
        clear_tables(&tx)?;
        write_meta(&tx, "schema_version", VECTOR_SCHEMA_VERSION.to_string())?;
        write_meta(&tx, "backend", &self.backend)?;
        write_meta(&tx, "model", &self.model)?;
        write_meta(&tx, "dimensions", self.dimensions.to_string())?;
        write_meta(&tx, "root", self.root.to_string_lossy())?;
        write_meta(&tx, "created_unix", self.created_unix.to_string())?;
        write_meta(&tx, "records", self.records.len().to_string())?;
        for record in &self.records {
            insert_record_row(&tx, record)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn summary_default(root: &Path) -> Result<Option<PersistentVectorSummary>> {
        let sqlite = Self::sqlite_path(root);
        if sqlite.exists() {
            return Self::summary(&sqlite).map(Some);
        }
        let json = Self::json_path(root);
        if json.exists() {
            let db = Self::load_json(&json)?;
            return Ok(Some(db.summary_at(&json, "Json")));
        }
        Ok(None)
    }

    pub fn summary(path: &Path) -> Result<PersistentVectorSummary> {
        if path.extension().and_then(|value| value.to_str()) != Some("sqlite") {
            let db = Self::load_json(path)?;
            return Ok(db.summary_at(path, "Json"));
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open vector DB {}", path.display()))?;
        create_schema(&conn)?;
        let version = meta_value(&conn, "schema_version")?
            .unwrap_or_else(|| "0".to_string())
            .parse::<u32>()
            .unwrap_or_default();
        let backend = meta_value(&conn, "backend")?.unwrap_or_else(|| "LocalHash".to_string());
        let model =
            meta_value(&conn, "model")?.unwrap_or_else(|| "local-hash-embedding".to_string());
        let dimensions = meta_value(&conn, "dimensions")?
            .unwrap_or_else(|| "1536".to_string())
            .parse::<usize>()
            .unwrap_or(1536);
        let root = PathBuf::from(meta_value(&conn, "root")?.unwrap_or_default());
        let created_unix = meta_value(&conn, "created_unix")?
            .unwrap_or_else(|| "0".to_string())
            .parse::<u64>()
            .unwrap_or_default();
        Ok(PersistentVectorSummary {
            path: path.to_path_buf(),
            storage: "SQLite".to_string(),
            version,
            backend,
            model,
            dimensions,
            root,
            created_unix,
            records: count_table(&conn, "vector_records")?,
        })
    }

    fn summary_at(&self, path: &Path, storage: &str) -> PersistentVectorSummary {
        PersistentVectorSummary {
            path: path.to_path_buf(),
            storage: storage.to_string(),
            version: self.version,
            backend: self.backend.clone(),
            model: self.model.clone(),
            dimensions: self.dimensions,
            root: self.root.clone(),
            created_unix: self.created_unix,
            records: self.records.len(),
        }
    }

    pub fn build(root: &Path, chunks: &[RichChunk], client: &EmbeddingClient) -> Result<Self> {
        let texts = chunks
            .iter()
            .map(RichChunk::embed_text)
            .collect::<Vec<String>>();
        let vectors = client.embed_texts(&texts)?;
        if vectors.len() != chunks.len() {
            anyhow::bail!(
                "embedding provider returned {} vectors for {} chunks",
                vectors.len(),
                chunks.len()
            );
        }

        let config = client.config();
        let records = chunks
            .iter()
            .cloned()
            .zip(vectors)
            .zip(texts)
            .map(|((chunk, vector), text)| VectorRecord {
                id: chunk.id.clone(),
                chunk,
                vector,
                text_hash: text_hash(&text),
            })
            .collect();

        Ok(Self {
            version: VECTOR_SCHEMA_VERSION,
            backend: format!("{:?}", config.backend),
            model: config.model.clone(),
            dimensions: config.dimensions,
            root: root.to_path_buf(),
            created_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            records,
            storage: "SQLite".to_string(),
            path: Some(Self::sqlite_path(root)),
        })
    }

    pub fn refresh_default(
        root: &Path,
        chunks: &[RichChunk],
        config: EmbeddingConfig,
        max_incremental_paths: usize,
    ) -> Result<(PersistentVectorRefreshResult, Self)> {
        let path = Self::sqlite_path(root);
        if !path.exists() {
            let client = EmbeddingClient::new(config);
            let db = Self::build(root, chunks, &client)?;
            let path = db.save_default(root)?;
            let summary = Self::summary(&path)?;
            let records_after = summary.records;
            return Ok((
                PersistentVectorRefreshResult {
                    mode: PersistentVectorRefreshMode::Rebuilt,
                    path,
                    touched_paths: unique_chunk_paths(chunks).len(),
                    records_before: 0,
                    records_after,
                    summary,
                },
                db,
            ));
        }

        let expected_backend = format!("{:?}", config.backend);
        let before_summary = Self::summary(&path)?;
        if before_summary.version != VECTOR_SCHEMA_VERSION
            || before_summary.backend != expected_backend
            || before_summary.model != config.model
            || before_summary.dimensions != config.dimensions
        {
            let client = EmbeddingClient::new(config);
            let db = Self::build(root, chunks, &client)?;
            db.save_default(root)?;
            let summary = Self::summary(&path)?;
            let loaded = Self::load_sqlite(&path)?;
            let records_after = summary.records;
            return Ok((
                PersistentVectorRefreshResult {
                    mode: PersistentVectorRefreshMode::Rebuilt,
                    path,
                    touched_paths: unique_chunk_paths(chunks).len(),
                    records_before: before_summary.records,
                    records_after,
                    summary,
                },
                loaded,
            ));
        }

        let mut conn = Connection::open(&path)
            .with_context(|| format!("failed to open vector DB {}", path.display()))?;
        create_schema(&conn)?;
        let indexed = load_record_headers(&conn)?;
        let current = current_record_headers(chunks);
        let touched = touched_paths(&indexed, &current);

        if touched.is_empty() {
            let db = Self::load_sqlite(&path)?;
            let summary = db.summary_at(&path, "SQLite");
            let records = summary.records;
            return Ok((
                PersistentVectorRefreshResult {
                    mode: PersistentVectorRefreshMode::Unchanged,
                    path,
                    touched_paths: 0,
                    records_before: records,
                    records_after: records,
                    summary,
                },
                db,
            ));
        }

        if touched.len() > max_incremental_paths {
            let client = EmbeddingClient::new(config);
            let db = Self::build(root, chunks, &client)?;
            db.save_default(root)?;
            let summary = Self::summary(&path)?;
            let loaded = Self::load_sqlite(&path)?;
            let records_after = summary.records;
            return Ok((
                PersistentVectorRefreshResult {
                    mode: PersistentVectorRefreshMode::Rebuilt,
                    path,
                    touched_paths: touched.len(),
                    records_before: before_summary.records,
                    records_after,
                    summary,
                },
                loaded,
            ));
        }

        let replacement = chunks
            .iter()
            .filter(|chunk| touched.contains(&chunk.path))
            .cloned()
            .collect::<Vec<_>>();
        let client = EmbeddingClient::new(config.clone());
        let replacement_texts = replacement
            .iter()
            .map(RichChunk::embed_text)
            .collect::<Vec<_>>();
        let vectors = client.embed_texts(&replacement_texts)?;
        if vectors.len() != replacement.len() {
            anyhow::bail!(
                "embedding provider returned {} vectors for {} chunks",
                vectors.len(),
                replacement.len()
            );
        }
        let records = replacement
            .into_iter()
            .zip(vectors)
            .zip(replacement_texts)
            .map(|((chunk, vector), text)| VectorRecord {
                id: chunk.id.clone(),
                chunk,
                vector,
                text_hash: text_hash(&text),
            })
            .collect::<Vec<_>>();

        let tx = conn.transaction()?;
        for rel in &touched {
            tx.execute(
                "DELETE FROM vector_records WHERE file_path = ?1",
                params![path_text(rel)],
            )?;
        }
        for record in &records {
            insert_record_row(&tx, record)?;
        }
        write_meta(&tx, "schema_version", VECTOR_SCHEMA_VERSION.to_string())?;
        write_meta(&tx, "backend", expected_backend)?;
        write_meta(&tx, "model", &config.model)?;
        write_meta(&tx, "dimensions", config.dimensions.to_string())?;
        write_meta(&tx, "root", root.to_string_lossy())?;
        write_meta(&tx, "created_unix", before_summary.created_unix.to_string())?;
        write_meta(
            &tx,
            "records",
            count_table(&tx, "vector_records")?.to_string(),
        )?;
        tx.commit()?;

        let summary = Self::summary(&path)?;
        let loaded = Self::load_sqlite(&path)?;
        let records_after = summary.records;
        Ok((
            PersistentVectorRefreshResult {
                mode: PersistentVectorRefreshMode::Incremental,
                path,
                touched_paths: touched.len(),
                records_before: before_summary.records,
                records_after,
                summary,
            },
            loaded,
        ))
    }

    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        client: &EmbeddingClient,
    ) -> Result<Vec<PersistentVectorHit>> {
        let query_vector = client.embed_one(query)?;
        Ok(self.search_vector(&query_vector, top_k))
    }

    pub fn search_vector(&self, query_vector: &[f32], top_k: usize) -> Vec<PersistentVectorHit> {
        let mut hits = self
            .records
            .iter()
            .filter_map(|record| {
                let score = cosine_dense(query_vector, &record.vector);
                (score > 0.0).then(|| PersistentVectorHit {
                    chunk: record.chunk.clone(),
                    score,
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.chunk.path.cmp(&right.chunk.path))
                .then_with(|| left.chunk.line_start.cmp(&right.chunk.line_start))
        });
        hits.truncate(top_k);
        hits
    }

    pub fn config(&self) -> EmbeddingConfig {
        match self.backend.as_str() {
            "OpenAi" => EmbeddingConfig::openai(Some(self.model.clone())),
            "OpenAiCompatible" => EmbeddingConfig::openai_compatible(
                std::env::var("CPL_EMBEDDING_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:11434/v1/embeddings".to_string()),
                self.model.clone(),
                self.dimensions,
            ),
            "Ollama" => EmbeddingConfig::ollama(Some(self.model.clone()), self.dimensions),
            _ => EmbeddingConfig::local_hash(self.dimensions),
        }
    }

    pub fn render_summary(&self) -> String {
        self.summary_at(
            self.path
                .as_deref()
                .unwrap_or_else(|| Path::new(".cpl/vectors.sqlite")),
            if self.storage.is_empty() {
                "SQLite"
            } else {
                &self.storage
            },
        )
        .render_human()
    }
}

pub fn build_and_save_default(
    root: &Path,
    chunks: &[RichChunk],
    config: EmbeddingConfig,
) -> Result<(PersistentVectorDb, PathBuf)> {
    let client = EmbeddingClient::new(config);
    let db = PersistentVectorDb::build(root, chunks, &client)?;
    let path = db.save_default(root)?;
    let mut db = db;
    db.path = Some(path.clone());
    Ok((db, path))
}

pub fn refresh_and_save_default(
    root: &Path,
    chunks: &[RichChunk],
    config: EmbeddingConfig,
    max_incremental_paths: usize,
) -> Result<(PersistentVectorRefreshResult, PersistentVectorDb)> {
    PersistentVectorDb::refresh_default(root, chunks, config, max_incremental_paths)
}

#[derive(Debug, Clone)]
struct RecordHeader {
    path: PathBuf,
    text_hash: String,
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS vector_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS vector_records (
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
            text_hash TEXT NOT NULL,
            vector BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_vector_records_file_path ON vector_records(file_path);
        CREATE INDEX IF NOT EXISTS idx_vector_records_text_hash ON vector_records(text_hash);",
    )?;
    Ok(())
}

fn clear_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM vector_meta;
        DELETE FROM vector_records;",
    )?;
    Ok(())
}

fn insert_record_row(conn: &Connection, record: &VectorRecord) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO vector_records
        (id, file_path, line_start, line_end, chunk_type, signature, source, docs,
         symbols_json, imports_json, module_path_json, text_hash, vector)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            record.id,
            path_text(&record.chunk.path),
            record.chunk.line_start as i64,
            record.chunk.line_end as i64,
            format!("{:?}", record.chunk.chunk_type),
            record.chunk.signature,
            record.chunk.source,
            record.chunk.docs,
            serde_json::to_string(&record.chunk.symbols)?,
            serde_json::to_string(&record.chunk.imports)?,
            serde_json::to_string(&record.chunk.module_path)?,
            record.text_hash,
            encode_vector(&record.vector),
        ],
    )?;
    Ok(())
}

fn load_records(conn: &Connection) -> Result<Vec<VectorRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, line_start, line_end, chunk_type, signature, source, docs,
                symbols_json, imports_json, module_path_json, text_hash, vector
         FROM vector_records
         ORDER BY file_path, line_start, id",
    )?;
    let rows = stmt.query_map([], |row| {
        let chunk_type: String = row.get(4)?;
        let symbols_json: String = row.get(8)?;
        let imports_json: String = row.get(9)?;
        let module_path_json: String = row.get(10)?;
        let vector: Vec<u8> = row.get(12)?;
        let id: String = row.get(0)?;
        Ok(VectorRecord {
            id: id.clone(),
            chunk: RichChunk {
                id,
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
            },
            text_hash: row.get(11)?,
            vector: decode_vector(&vector),
        })
    })?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

fn load_record_headers(conn: &Connection) -> Result<BTreeMap<String, RecordHeader>> {
    let mut stmt = conn.prepare("SELECT id, file_path, text_hash FROM vector_records")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            RecordHeader {
                path: PathBuf::from(row.get::<_, String>(1)?),
                text_hash: row.get(2)?,
            },
        ))
    })?;
    let mut headers = BTreeMap::new();
    for row in rows {
        let (id, header) = row?;
        headers.insert(id, header);
    }
    Ok(headers)
}

fn current_record_headers(chunks: &[RichChunk]) -> BTreeMap<String, RecordHeader> {
    chunks
        .iter()
        .map(|chunk| {
            (
                chunk.id.clone(),
                RecordHeader {
                    path: chunk.path.clone(),
                    text_hash: text_hash(&chunk.embed_text()),
                },
            )
        })
        .collect()
}

fn touched_paths(
    indexed: &BTreeMap<String, RecordHeader>,
    current: &BTreeMap<String, RecordHeader>,
) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for (id, current_header) in current {
        match indexed.get(id) {
            Some(indexed_header) if indexed_header.text_hash == current_header.text_hash => {}
            _ => {
                paths.insert(current_header.path.clone());
            }
        }
    }
    for (id, indexed_header) in indexed {
        if !current.contains_key(id) {
            paths.insert(indexed_header.path.clone());
        }
    }
    paths
}

fn unique_chunk_paths(chunks: &[RichChunk]) -> BTreeSet<PathBuf> {
    chunks.iter().map(|chunk| chunk.path.clone()).collect()
}

fn write_meta(conn: &Connection, key: &str, value: impl AsRef<str>) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO vector_meta (key, value) VALUES (?1, ?2)",
        params![key, value.as_ref()],
    )?;
    Ok(())
}

fn meta_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM vector_meta WHERE key = ?1",
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

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::chunk::ChunkType;
    use crate::embedding::EmbeddingConfig;

    use super::*;

    #[test]
    fn builds_and_searches_sqlite_persistent_db() {
        let root = temp_root("sqlite_vector_db");
        fs::create_dir_all(&root).unwrap();
        let chunks = vec![sample_chunk("a", "src/auth.rs", "fn validate_token() {}")];
        let (db, path) =
            build_and_save_default(&root, &chunks, EmbeddingConfig::local_hash(128)).unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), "vectors.sqlite");
        let loaded = PersistentVectorDb::load_default(&root).unwrap().unwrap();
        let client = EmbeddingClient::new(loaded.config());
        let hits = db.search("validate auth token", 1, &client).unwrap();
        assert_eq!(hits[0].chunk.path, PathBuf::from("src/auth.rs"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_updates_only_changed_path() {
        let root = temp_root("sqlite_vector_refresh");
        fs::create_dir_all(&root).unwrap();
        let chunks = vec![
            sample_chunk("a", "src/auth.rs", "fn validate_token() {}"),
            sample_chunk("b", "src/db.rs", "fn save_user() {}"),
        ];
        let (_db, _path) =
            build_and_save_default(&root, &chunks, EmbeddingConfig::local_hash(128)).unwrap();
        let updated = vec![
            sample_chunk("a2", "src/auth.rs", "fn login_user() {}"),
            sample_chunk("b", "src/db.rs", "fn save_user() {}"),
        ];
        let (result, db) =
            refresh_and_save_default(&root, &updated, EmbeddingConfig::local_hash(128), 8).unwrap();
        assert_eq!(result.mode, PersistentVectorRefreshMode::Incremental);
        assert_eq!(result.touched_paths, 1);
        assert_eq!(db.records.len(), 2);
        let client = EmbeddingClient::new(db.config());
        assert_eq!(
            db.search("login user", 1, &client).unwrap()[0].chunk.path,
            PathBuf::from("src/auth.rs")
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn sample_chunk(id: &str, path: &str, source: &str) -> RichChunk {
        RichChunk {
            id: id.to_string(),
            path: PathBuf::from(path),
            source: source.to_string(),
            signature: Some(source.to_string()),
            docs: None,
            chunk_type: ChunkType::Function,
            symbols: vec![id.to_string()],
            imports: Vec::new(),
            module_path: vec!["src".to_string()],
            line_start: 1,
            line_end: 1,
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cpl_vec_{name}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
