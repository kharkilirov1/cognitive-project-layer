use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::chunk::RichChunk;
use crate::embedding::{EmbeddingClient, EmbeddingConfig, cosine_dense, text_hash};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentVectorDb {
    pub version: u32,
    pub backend: String,
    pub model: String,
    pub dimensions: usize,
    pub root: PathBuf,
    pub created_unix: u64,
    pub records: Vec<VectorRecord>,
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

impl PersistentVectorDb {
    pub fn default_path(root: &Path) -> PathBuf {
        root.join(".cpl").join("vector_db.json")
    }

    pub fn load_default(root: &Path) -> Result<Option<Self>> {
        let path = Self::default_path(root);
        if !path.exists() {
            return Ok(None);
        }
        Self::load(&path).map(Some)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read vector db {}", path.display()))?;
        Ok(serde_json::from_str(&source)?)
    }

    pub fn save_default(&self, root: &Path) -> Result<PathBuf> {
        let path = Self::default_path(root);
        self.save(&path)?;
        Ok(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
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
            version: 1,
            backend: format!("{:?}", config.backend),
            model: config.model.clone(),
            dimensions: config.dimensions,
            root: root.to_path_buf(),
            created_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            records,
        })
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
        format!(
            "Persistent vector DB\nBackend: {}\nModel: {}\nDimensions: {}\nRecords: {}\nRoot: {}",
            self.backend,
            self.model,
            self.dimensions,
            self.records.len(),
            self.root.display()
        )
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
    Ok((db, path))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::chunk::ChunkType;
    use crate::embedding::EmbeddingConfig;

    use super::*;

    #[test]
    fn builds_and_searches_local_persistent_db() {
        let root = std::env::temp_dir().join(format!(
            "cpl_vec_test_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let chunks = vec![RichChunk {
            id: "src/auth.rs::symbol::validate_token:1-3".to_string(),
            path: PathBuf::from("src/auth.rs"),
            source: "fn validate_token() {}".to_string(),
            signature: Some("fn validate_token()".to_string()),
            docs: None,
            chunk_type: ChunkType::Function,
            symbols: vec!["validate_token".to_string()],
            imports: Vec::new(),
            module_path: vec!["src".to_string()],
            line_start: 1,
            line_end: 3,
        }];
        let (db, path) =
            build_and_save_default(&root, &chunks, EmbeddingConfig::local_hash(128)).unwrap();
        assert!(path.exists());
        let loaded = PersistentVectorDb::load_default(&root).unwrap().unwrap();
        let client = EmbeddingClient::new(loaded.config());
        let hits = db.search("validate auth token", 1, &client).unwrap();
        assert_eq!(hits[0].chunk.path, PathBuf::from("src/auth.rs"));
        fs::remove_dir_all(root).unwrap();
    }
}
