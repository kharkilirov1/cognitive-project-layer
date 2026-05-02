use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::graph::ProjectGraph;
use crate::scanner::ProjectScan;

/// Реальные состояния индексации, влияющие на поведение retrieval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexState {
    /// Только скан, нет графа/векторов
    Cold,
    /// Есть skeleton, граф строится
    Warming,
    /// Полностью проиндексирован
    Hot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LazyIndexer {
    pub state: IndexState,
    pub touched_files: BTreeSet<PathBuf>,
    pub related_files: BTreeSet<PathBuf>,
    pub warm_files: BTreeSet<PathBuf>,
    pub total_source_files: usize,
    pub indexed_source_files: usize,
    pub graph_edges: usize,
    pub vector_chunks: usize,
}

impl LazyIndexer {
    pub fn cold(total_source_files: usize) -> Self {
        Self {
            state: IndexState::Cold,
            touched_files: BTreeSet::new(),
            related_files: BTreeSet::new(),
            warm_files: BTreeSet::new(),
            total_source_files,
            indexed_source_files: 0,
            graph_edges: 0,
            vector_chunks: 0,
        }
    }

    pub fn skeleton(scan: &ProjectScan) -> Self {
        let mut indexer = Self::cold(scan.source_files);
        indexer.state = IndexState::Warming;
        for path in scan
            .entry_candidates
            .iter()
            .chain(scan.config_files.iter())
            .chain(scan.recent_changed_files.iter())
        {
            indexer.warm_files.insert(path.clone());
        }
        indexer.indexed_source_files = indexer.warm_files.len().min(scan.source_files);
        indexer
    }

    pub fn mark_touched(&mut self, path: PathBuf, graph: &ProjectGraph) {
        self.touched_files.insert(path.clone());
        self.warm_files.insert(path.clone());
        for related in graph.expand_files(&[path], 1) {
            self.related_files.insert(related.clone());
            self.warm_files.insert(related);
        }
        self.indexed_source_files = self.warm_files.len().min(self.total_source_files);
    }

    pub fn mark_hot(&mut self, graph: &ProjectGraph, vector_chunks: usize) {
        self.state = IndexState::Hot;
        self.graph_edges = graph.edges.len();
        self.vector_chunks = vector_chunks;
        self.indexed_source_files = self.total_source_files;
    }

    pub fn refresh_file(&mut self, path: PathBuf, graph: &ProjectGraph) {
        self.mark_touched(path, graph);
        if self.indexed_source_files >= self.total_source_files {
            self.mark_hot(graph, self.vector_chunks);
        }
    }

    /// Возвращает true, если можно использовать полный RAG (векторный поиск).
    pub fn can_use_rag(&self) -> bool {
        self.state >= IndexState::Warming && self.vector_chunks > 0
    }

    /// Возвращает true, если граф построен.
    pub fn has_graph(&self) -> bool {
        self.graph_edges > 0
    }

    pub fn render_status(&self) -> String {
        format!(
            "Indexer: {:?}\nSource files: {}/{}\nTouched: {}\nRelated: {}\nWarm: {}\nGraph edges: {}\nVector chunks: {}",
            self.state,
            self.indexed_source_files,
            self.total_source_files,
            self.touched_files.len(),
            self.related_files.len(),
            self.warm_files.len(),
            self.graph_edges,
            self.vector_chunks
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_state_starts_empty() {
        let indexer = LazyIndexer::cold(10);
        assert_eq!(indexer.state, IndexState::Cold);
        assert_eq!(indexer.indexed_source_files, 0);
    }

    #[test]
    fn hot_state_can_use_rag() {
        let mut indexer = LazyIndexer::cold(10);
        indexer.state = IndexState::Hot;
        indexer.vector_chunks = 5;
        assert!(indexer.can_use_rag());
    }

    #[test]
    fn cold_state_cannot_use_rag() {
        let indexer = LazyIndexer::cold(10);
        assert!(!indexer.can_use_rag());
    }
}
