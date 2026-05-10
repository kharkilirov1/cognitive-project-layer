use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::config::ProjectConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EmbeddingBackend {
    OpenAi,
    OpenAiCompatible,
    Ollama,
    LocalHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub backend: EmbeddingBackend,
    pub model: String,
    pub endpoint: Option<String>,
    pub dimensions: usize,
}

impl EmbeddingConfig {
    pub fn openai(model: Option<String>) -> Self {
        Self {
            backend: EmbeddingBackend::OpenAi,
            model: model.unwrap_or_else(|| "text-embedding-3-small".to_string()),
            endpoint: Some("https://api.openai.com/v1/embeddings".to_string()),
            dimensions: 1536,
        }
    }

    pub fn openai_compatible(endpoint: String, model: String, dimensions: usize) -> Self {
        Self {
            backend: EmbeddingBackend::OpenAiCompatible,
            model,
            endpoint: Some(endpoint),
            dimensions,
        }
    }

    pub fn ollama(model: Option<String>, dimensions: usize) -> Self {
        Self {
            backend: EmbeddingBackend::Ollama,
            model: model.unwrap_or_else(|| "nomic-embed-text".to_string()),
            endpoint: Some("http://localhost:11434/v1/embeddings".to_string()),
            dimensions,
        }
    }

    pub fn local_hash(dimensions: usize) -> Self {
        Self {
            backend: EmbeddingBackend::LocalHash,
            model: "local-hash-embedding".to_string(),
            endpoint: None,
            dimensions,
        }
    }

    pub fn from_env_or_local() -> Self {
        Self::from_config_or_env(ProjectConfig::default())
    }

    pub fn from_project_or_env(root: &Path) -> Self {
        Self::from_config_or_env(ProjectConfig::load(root))
    }

    fn from_config_or_env(project: ProjectConfig) -> Self {
        let backend = std::env::var("CPL_EMBEDDING_BACKEND")
            .ok()
            .or(project.embedding_backend)
            .unwrap_or_else(|| "local-hash".to_string())
            .to_ascii_lowercase();
        let dimensions = std::env::var("CPL_EMBEDDING_DIMENSIONS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .or(project.embedding_dimensions)
            .unwrap_or(1536);
        let model = || {
            std::env::var("CPL_EMBEDDING_MODEL")
                .ok()
                .or(project.embedding_model.clone())
        };

        match backend.as_str() {
            "openai" => Self::openai(model()),
            "ollama" => Self::ollama(model(), dimensions),
            "openai-compatible" | "local-model" | "local" => Self::openai_compatible(
                std::env::var("CPL_EMBEDDING_ENDPOINT")
                    .ok()
                    .or(project.embedding_endpoint)
                    .unwrap_or_else(|| "http://localhost:11434/v1/embeddings".to_string()),
                model().unwrap_or_else(|| "nomic-embed-text".to_string()),
                dimensions,
            ),
            _ => Self::local_hash(dimensions),
        }
    }
}

pub struct EmbeddingClient {
    config: EmbeddingConfig,
    http: Client,
}

impl EmbeddingClient {
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    pub fn config(&self) -> &EmbeddingConfig {
        &self.config
    }

    pub fn embed_texts(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        match self.config.backend {
            EmbeddingBackend::LocalHash => Ok(inputs
                .iter()
                .map(|input| local_hash_embedding(input, self.config.dimensions))
                .collect()),
            EmbeddingBackend::OpenAi
            | EmbeddingBackend::OpenAiCompatible
            | EmbeddingBackend::Ollama => self.embed_openai_compatible(inputs),
        }
    }

    pub fn embed_one(&self, input: &str) -> Result<Vec<f32>> {
        self.embed_texts(&[input.to_string()])?
            .into_iter()
            .next()
            .context("embedding provider returned no vectors")
    }

    fn embed_openai_compatible(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .context("embedding endpoint is not configured")?;
        let api_key = match self.config.backend {
            EmbeddingBackend::OpenAi => std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY is required for OpenAI embeddings")?,
            EmbeddingBackend::OpenAiCompatible => std::env::var("CPL_EMBEDDING_API_KEY")
                .or_else(|_| std::env::var("OPENAI_API_KEY"))
                .unwrap_or_default(),
            EmbeddingBackend::Ollama | EmbeddingBackend::LocalHash => String::new(),
        };

        let mut all = Vec::with_capacity(inputs.len());
        for batch in inputs.chunks(64) {
            let request = EmbeddingRequest {
                model: self.config.model.clone(),
                input: batch.to_vec(),
                encoding_format: "float".to_string(),
            };
            let mut builder = self.http.post(endpoint).json(&request);
            if !api_key.is_empty() {
                builder = builder.bearer_auth(&api_key);
            }
            let response = builder
                .send()
                .with_context(|| embedding_endpoint_help(endpoint, &self.config))?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().unwrap_or_default();
                anyhow::bail!(
                    "{}",
                    embedding_status_help(status.as_u16(), &body, &self.config)
                );
            }
            let mut response: EmbeddingResponse = response.json()?;
            response.data.sort_by_key(|item| item.index);
            all.extend(
                response
                    .data
                    .into_iter()
                    .map(|item| normalize_dense(item.embedding)),
            );
        }
        Ok(all)
    }
}

fn embedding_endpoint_help(endpoint: &str, config: &EmbeddingConfig) -> String {
    match config.backend {
        EmbeddingBackend::Ollama => format!(
            "failed to call Ollama embeddings endpoint {endpoint}. \
Ollama may not be running; run `ollama serve`, then `ollama pull {}`. \
Offline fallback: use `--backend local-hash` or `CPL_EMBEDDING_BACKEND=local-hash`.",
            config.model
        ),
        EmbeddingBackend::OpenAi => {
            "failed to call OpenAI embeddings endpoint; check network and OPENAI_API_KEY"
                .to_string()
        }
        EmbeddingBackend::OpenAiCompatible => format!(
            "failed to call embeddings endpoint {endpoint}; check CPL_EMBEDDING_ENDPOINT, model `{}`, and API key if required. Fallback: `--backend local-hash`.",
            config.model
        ),
        EmbeddingBackend::LocalHash => {
            "local-hash embeddings do not use an HTTP endpoint".to_string()
        }
    }
}

fn embedding_status_help(status: u16, body: &str, config: &EmbeddingConfig) -> String {
    match config.backend {
        EmbeddingBackend::Ollama => format!(
            "Ollama embedding request failed with HTTP {status}: {body}. \
If the model is missing, run `ollama pull {}`; if Ollama is stopped, run `ollama serve`; \
fallback: `--backend local-hash`.",
            config.model
        ),
        _ => format!("embedding request failed with HTTP {status}: {body}"),
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
    encoding_format: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingObject>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingObject {
    embedding: Vec<f32>,
    index: usize,
}

pub fn local_hash_embedding(text: &str, dimensions: usize) -> Vec<f32> {
    let dimensions = dimensions.max(8);
    let mut vector = vec![0.0f32; dimensions];
    for token in crate::chunk::tokenize_code_text(text) {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let digest = hasher.finalize();
        let idx = u64::from_le_bytes(digest[0..8].try_into().unwrap()) as usize % dimensions;
        let sign = if digest[8] & 1 == 0 { 1.0 } else { -1.0 };
        vector[idx] += sign;
    }
    normalize_dense(vector)
}

pub fn normalize_dense(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

pub fn cosine_dense(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum()
}

pub fn text_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hash_embedding_is_normalized() {
        let vector = local_hash_embedding("validate token auth", 64);
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }
}
