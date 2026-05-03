use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::chunk::RichChunk;
use crate::embedding::EmbeddingClient;
use crate::persistent_vector::{PersistentVectorDb, VectorRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QdrantConfig {
    pub url: String,
    pub collection: String,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl QdrantConfig {
    pub fn new(url: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            url: trim_trailing_slash(url.into()),
            collection: collection.into(),
            api_key: None,
            timeout_secs: 60,
        }
    }

    pub fn from_env() -> Self {
        let mut config = Self::new(
            std::env::var("CPL_QDRANT_URL").unwrap_or_else(|_| "http://localhost:6333".into()),
            std::env::var("CPL_QDRANT_COLLECTION")
                .unwrap_or_else(|_| "cognitive_project_layer".into()),
        );
        config.api_key = std::env::var("QDRANT_API_KEY")
            .or_else(|_| std::env::var("CPL_QDRANT_API_KEY"))
            .ok();
        config.timeout_secs = std::env::var("CPL_QDRANT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(60);
        config
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QdrantHit {
    pub id: String,
    pub score: f32,
    pub chunk: RichChunk,
}

pub struct QdrantVectorClient {
    config: QdrantConfig,
    http: Client,
}

impl QdrantVectorClient {
    pub fn new(config: QdrantConfig) -> Result<Self> {
        validate_collection_name(&config.collection)?;
        let http = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .context("failed to create qdrant http client")?;
        Ok(Self { config, http })
    }

    pub fn ensure_collection(&self, dimensions: usize) -> Result<()> {
        let body = qdrant_collection_body(dimensions);
        let response = self
            .request(
                self.http
                    .put(self.endpoint(&format!("/collections/{}", self.config.collection))),
            )
            .json(&body)
            .send()
            .context("failed to create/update qdrant collection")?;
        accept_qdrant_response(response, "create/update qdrant collection")?;
        Ok(())
    }

    pub fn upsert_db(&self, db: &PersistentVectorDb, batch_size: usize) -> Result<usize> {
        if db.records.is_empty() && db.record_count() > 0 {
            anyhow::bail!(
                "persistent vector DB records are not loaded; load it eagerly before Qdrant upsert"
            );
        }
        self.ensure_collection(db.dimensions)?;
        let batch_size = batch_size.max(1);
        let mut total = 0usize;
        for batch in db.records.chunks(batch_size) {
            let body = json!({
                "points": batch.iter().map(qdrant_point).collect::<Vec<_>>()
            });
            let response = self
                .request(self.http.put(self.endpoint(&format!(
                    "/collections/{}/points?wait=true",
                    self.config.collection
                ))))
                .json(&body)
                .send()
                .context("failed to upsert qdrant points")?;
            accept_qdrant_response(response, "upsert qdrant points")?;
            total += batch.len();
        }
        Ok(total)
    }

    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        embeddings: &EmbeddingClient,
    ) -> Result<Vec<QdrantHit>> {
        let vector = embeddings.embed_one(query)?;
        self.search_vector(&vector, top_k)
    }

    pub fn search_vector(&self, vector: &[f32], top_k: usize) -> Result<Vec<QdrantHit>> {
        let body = qdrant_query_body(vector, top_k);
        let response = self
            .request(self.http.post(self.endpoint(&format!(
                "/collections/{}/points/query",
                self.config.collection
            ))))
            .json(&body)
            .send()
            .context("failed to query qdrant points")?;
        let response = accept_qdrant_response(response, "query qdrant points")?;
        qdrant_hits_from_response(response)
    }

    pub fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.config.url, path)
    }

    fn request(&self, builder: RequestBuilder) -> RequestBuilder {
        if let Some(api_key) = &self.config.api_key {
            builder.header("api-key", api_key)
        } else {
            builder
        }
    }
}

fn qdrant_collection_body(dimensions: usize) -> Value {
    json!({
        "vectors": {
            "size": dimensions,
            "distance": "Cosine"
        }
    })
}

fn qdrant_query_body(vector: &[f32], top_k: usize) -> Value {
    json!({
        "query": vector,
        "limit": top_k.max(1),
        "with_payload": true,
        "with_vector": false
    })
}

fn qdrant_point(record: &VectorRecord) -> Value {
    json!({
        "id": deterministic_uuid(&record.id),
        "vector": record.vector,
        "payload": {
            "record_id": record.id,
            "text_hash": record.text_hash,
            "path": record.chunk.path.to_string_lossy().replace('\\', "/"),
            "line_start": record.chunk.line_start,
            "line_end": record.chunk.line_end,
            "symbols": record.chunk.symbols,
            "chunk": record.chunk,
        }
    })
}

fn qdrant_hits_from_response(value: Value) -> Result<Vec<QdrantHit>> {
    let points = value
        .get("result")
        .and_then(|result| result.get("points"))
        .and_then(Value::as_array)
        .context("qdrant response has no result.points array")?;

    let mut hits = Vec::new();
    for point in points {
        let payload = point.get("payload").cloned().unwrap_or_else(|| json!({}));
        let chunk_value = payload
            .get("chunk")
            .cloned()
            .context("qdrant point payload has no chunk object")?;
        let chunk = serde_json::from_value::<RichChunk>(chunk_value)?;
        let id = point
            .get("id")
            .map(id_to_string)
            .unwrap_or_else(|| "unknown".to_string());
        let score = point
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or_default() as f32;
        hits.push(QdrantHit { id, score, chunk });
    }
    Ok(hits)
}

fn accept_qdrant_response(response: reqwest::blocking::Response, action: &str) -> Result<Value> {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("{action} failed with {status}: {body}");
    }
    Ok(serde_json::from_str(&body).unwrap_or_else(|_| json!({})))
}

fn deterministic_uuid(value: &str) -> String {
    let mut digest = Sha256::digest(value.as_bytes());
    digest[6] = (digest[6] & 0x0f) | 0x50;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0],
        digest[1],
        digest[2],
        digest[3],
        digest[4],
        digest[5],
        digest[6],
        digest[7],
        digest[8],
        digest[9],
        digest[10],
        digest[11],
        digest[12],
        digest[13],
        digest[14],
        digest[15],
    )
}

fn id_to_string(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
        .unwrap_or_else(|| value.to_string())
}

fn trim_trailing_slash(mut url: String) -> String {
    while url.ends_with('/') {
        url.pop();
    }
    url
}

fn validate_collection_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        anyhow::bail!("invalid qdrant collection name `{name}`; use letters, digits, `_`, `-`");
    }
    Ok(())
}

#[allow(dead_code)]
fn _keep_pathbuf_for_docs(_: PathBuf) {}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::chunk::{ChunkType, RichChunk};

    use super::*;

    #[test]
    fn deterministic_uuid_is_stable_and_uuid_shaped() {
        let first = deterministic_uuid("src/lib.rs::symbol::retrieve:1-5");
        let second = deterministic_uuid("src/lib.rs::symbol::retrieve:1-5");
        assert_eq!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(first.matches('-').count(), 4);
    }

    #[test]
    fn query_body_uses_current_qdrant_query_api() {
        let body = qdrant_query_body(&[0.1, 0.2, 0.3], 5);
        assert_eq!(body["query"].as_array().unwrap().len(), 3);
        assert_eq!(body["limit"], 5);
        assert_eq!(body["with_payload"], true);
    }

    #[test]
    fn parses_query_response_payload_chunk() {
        let chunk = RichChunk {
            id: "a".to_string(),
            path: PathBuf::from("src/auth.rs"),
            source: "fn login() {}".to_string(),
            signature: Some("fn login()".to_string()),
            docs: None,
            chunk_type: ChunkType::Function,
            symbols: vec!["login".to_string()],
            imports: Vec::new(),
            module_path: vec!["src".to_string()],
            line_start: 1,
            line_end: 1,
        };
        let response = json!({
            "result": {
                "points": [{
                    "id": deterministic_uuid("a"),
                    "score": 0.7,
                    "payload": {"chunk": chunk}
                }]
            }
        });
        let hits = qdrant_hits_from_response(response).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.path, PathBuf::from("src/auth.rs"));
    }
}
