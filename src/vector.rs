use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::chunk::{RichChunk, tokenize_code_text};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorHit {
    pub chunk: RichChunk,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VectorStore {
    pub chunks: Vec<RichChunk>,
    vectors: Vec<BTreeMap<String, f32>>,
    document_frequency: BTreeMap<String, usize>,
}

impl VectorStore {
    pub fn build(chunks: Vec<RichChunk>) -> Self {
        let mut vectors = Vec::with_capacity(chunks.len());
        let mut document_frequency = BTreeMap::<String, usize>::new();

        for chunk in &chunks {
            let counts = token_counts(&chunk.embed_text());
            for token in counts.keys().cloned().collect::<BTreeSet<_>>() {
                *document_frequency.entry(token).or_insert(0) += 1;
            }
            vectors.push(normalize(counts));
        }

        Self {
            chunks,
            vectors,
            document_frequency,
        }
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn replace_path_chunks(&mut self, path: &std::path::Path, replacement: Vec<RichChunk>) {
        let mut retained = Vec::with_capacity(self.chunks.len() + replacement.len());
        for (chunk, vector) in self.chunks.drain(..).zip(self.vectors.drain(..)) {
            if chunk.path == path {
                for token in vector.keys() {
                    decrement_document_frequency(&mut self.document_frequency, token);
                }
            } else {
                retained.push((chunk, vector));
            }
        }

        for chunk in replacement {
            let counts = token_counts(&chunk.embed_text());
            for token in counts.keys().cloned().collect::<BTreeSet<_>>() {
                *self.document_frequency.entry(token).or_insert(0) += 1;
            }
            retained.push((chunk, normalize(counts)));
        }

        retained.sort_by(|left, right| {
            left.0
                .path
                .cmp(&right.0.path)
                .then_with(|| left.0.line_start.cmp(&right.0.line_start))
        });
        self.chunks = retained.iter().map(|(chunk, _)| chunk.clone()).collect();
        self.vectors = retained.into_iter().map(|(_, vector)| vector).collect();
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<VectorHit> {
        if self.is_empty() || query.trim().is_empty() {
            return Vec::new();
        }

        let mut query_counts = BTreeMap::<String, f32>::new();
        for token in tokenize_code_text(query) {
            *query_counts.entry(token).or_insert(0.0) += 1.0;
        }
        let query_vector = normalize(tf_idf(
            query_counts,
            &self.document_frequency,
            self.chunks.len().max(1) as f32,
        ));

        let mut hits = self
            .vectors
            .iter()
            .enumerate()
            .filter_map(|(idx, vector)| {
                let score = dot(&query_vector, vector);
                (score > 0.0).then(|| VectorHit {
                    chunk: self.chunks[idx].clone(),
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
}

fn token_counts(text: &str) -> BTreeMap<String, f32> {
    let mut counts = BTreeMap::<String, f32>::new();
    for token in tokenize_code_text(text) {
        *counts.entry(token).or_insert(0.0) += 1.0;
    }
    counts
}

fn decrement_document_frequency(document_frequency: &mut BTreeMap<String, usize>, token: &str) {
    if let Some(count) = document_frequency.get_mut(token) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            document_frequency.remove(token);
        }
    }
}

fn tf_idf(
    counts: BTreeMap<String, f32>,
    document_frequency: &BTreeMap<String, usize>,
    total_docs: f32,
) -> BTreeMap<String, f32> {
    counts
        .into_iter()
        .map(|(token, count)| {
            let df = *document_frequency.get(&token).unwrap_or(&0) as f32;
            let idf = ((total_docs + 1.0) / (df + 1.0)).ln() + 1.0;
            (token, count.sqrt() * idf)
        })
        .collect()
}

fn normalize(mut vector: BTreeMap<String, f32>) -> BTreeMap<String, f32> {
    let norm = vector
        .values()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm > 0.0 {
        for value in vector.values_mut() {
            *value /= norm;
        }
    }
    vector
}

fn dot(left: &BTreeMap<String, f32>, right: &BTreeMap<String, f32>) -> f32 {
    if left.len() <= right.len() {
        left.iter()
            .map(|(token, value)| value * right.get(token).copied().unwrap_or(0.0))
            .sum()
    } else {
        right
            .iter()
            .map(|(token, value)| value * left.get(token).copied().unwrap_or(0.0))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::chunk::ChunkType;

    use super::*;

    #[test]
    fn finds_relevant_chunk() {
        let chunks = vec![
            RichChunk {
                id: "a".to_string(),
                path: PathBuf::from("src/auth.rs"),
                source: "fn validate_token() {}".to_string(),
                signature: Some("fn validate_token()".to_string()),
                docs: None,
                chunk_type: ChunkType::Function,
                symbols: vec!["validate_token".to_string()],
                imports: Vec::new(),
                module_path: vec!["src".to_string()],
                line_start: 1,
                line_end: 1,
            },
            RichChunk {
                id: "b".to_string(),
                path: PathBuf::from("src/db.rs"),
                source: "fn save_user() {}".to_string(),
                signature: Some("fn save_user()".to_string()),
                docs: None,
                chunk_type: ChunkType::Function,
                symbols: vec!["save_user".to_string()],
                imports: Vec::new(),
                module_path: vec!["src".to_string()],
                line_start: 1,
                line_end: 1,
            },
        ];
        let store = VectorStore::build(chunks);
        let hits = store.search("validate token auth", 1);
        assert_eq!(hits[0].chunk.path, PathBuf::from("src/auth.rs"));
    }

    #[test]
    fn replaces_only_chunks_for_path() {
        let chunks = vec![
            RichChunk {
                id: "a".to_string(),
                path: PathBuf::from("src/auth.rs"),
                source: "fn validate_token() {}".to_string(),
                signature: Some("fn validate_token()".to_string()),
                docs: None,
                chunk_type: ChunkType::Function,
                symbols: vec!["validate_token".to_string()],
                imports: Vec::new(),
                module_path: vec!["src".to_string()],
                line_start: 1,
                line_end: 1,
            },
            RichChunk {
                id: "b".to_string(),
                path: PathBuf::from("src/db.rs"),
                source: "fn save_user() {}".to_string(),
                signature: Some("fn save_user()".to_string()),
                docs: None,
                chunk_type: ChunkType::Function,
                symbols: vec!["save_user".to_string()],
                imports: Vec::new(),
                module_path: vec!["src".to_string()],
                line_start: 1,
                line_end: 1,
            },
        ];
        let mut store = VectorStore::build(chunks);
        store.replace_path_chunks(
            &PathBuf::from("src/auth.rs"),
            vec![RichChunk {
                id: "a2".to_string(),
                path: PathBuf::from("src/auth.rs"),
                source: "fn login_user() {}".to_string(),
                signature: Some("fn login_user()".to_string()),
                docs: None,
                chunk_type: ChunkType::Function,
                symbols: vec!["login_user".to_string()],
                imports: Vec::new(),
                module_path: vec!["src".to_string()],
                line_start: 1,
                line_end: 1,
            }],
        );

        assert_eq!(store.len(), 2);
        assert_eq!(store.search("login user", 1)[0].chunk.id, "a2");
        assert_eq!(store.search("save user", 1)[0].chunk.id, "b");
    }
}
