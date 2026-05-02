use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::retrieval::RetrievalResult;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkingMemory {
    pub current_task: Option<String>,
    pub opened_files: Vec<PathBuf>,
    pub relevant_symbols: Vec<String>,
    pub confirmed_facts: Vec<String>,
    pub hypotheses: Vec<String>,
    pub rejected_paths: Vec<PathBuf>,
    pub pending_questions: Vec<String>,
}

impl WorkingMemory {
    pub fn remember_retrieval(&mut self, query: &str, result: &RetrievalResult) {
        self.current_task = Some(query.to_string());

        for chunk in &result.chunks {
            // LRU: если файл уже есть, перемещаем в конец
            if let Some(pos) = self.opened_files.iter().position(|p| *p == chunk.path) {
                self.opened_files.remove(pos);
            }
            self.opened_files.push(chunk.path.clone());

            for symbol in &chunk.symbols {
                // LRU для символов
                if let Some(pos) = self.relevant_symbols.iter().position(|s| s == symbol) {
                    self.relevant_symbols.remove(pos);
                }
                self.relevant_symbols.push(symbol.clone());
            }
        }

        // LRU eviction: удаляем самые старые (первые в списке)
        while self.opened_files.len() > 50 {
            self.opened_files.remove(0);
        }
        while self.relevant_symbols.len() > 100 {
            self.relevant_symbols.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::{CandidateChunk, QueryAnalysis, QueryIntent};

    fn make_result(paths: &[&str]) -> RetrievalResult {
        let chunks = paths
            .iter()
            .map(|p| CandidateChunk {
                path: PathBuf::from(p),
                line_start: 1,
                line_end: 10,
                preview: String::new(),
                symbols: vec![],
                symbol_score: 0.0,
                lexical_score: 0.0,
                semantic_score: 0.0,
                graph_score: 0.0,
                recency_score: 0.0,
                final_score: 0.0,
                reasons: vec![],
            })
            .collect();
        RetrievalResult {
            query: QueryAnalysis {
                raw: "test".to_string(),
                intent: QueryIntent::Unknown,
                symbols: vec![],
                terms: vec![],
                module_hints: vec![],
            },
            chunks,
            confidence: 0.5,
            strategy: crate::confidence::RetrievalStrategy::Hybrid,
            reasons: vec![],
            skeleton_prompt: String::new(),
            fallback_plan: vec![],
        }
    }

    #[test]
    fn lru_eviction_removes_oldest() {
        let mut memory = WorkingMemory::default();
        let result = make_result(&["a.rs", "b.rs", "c.rs"]);
        memory.remember_retrieval("q1", &result);
        assert_eq!(memory.opened_files.len(), 3);

        // Добавляем новые файлы, старые должны вытесняться
        for i in 0..60 {
            let result = make_result(&[&format!("file_{}.rs", i)]);
            memory.remember_retrieval(&format!("q{}", i), &result);
        }
        assert_eq!(memory.opened_files.len(), 50);
        // Самый старый "a.rs" должен быть вытеснен
        assert!(!memory.opened_files.contains(&PathBuf::from("a.rs")));
    }

    #[test]
    fn lru_moves_existing_to_end() {
        let mut memory = WorkingMemory::default();
        let result = make_result(&["a.rs", "b.rs"]);
        memory.remember_retrieval("q1", &result);

        // Повторно обращаемся к a.rs — он должен переместиться в конец
        let result = make_result(&["a.rs"]);
        memory.remember_retrieval("q2", &result);
        assert_eq!(memory.opened_files.last(), Some(&PathBuf::from("a.rs")));
    }
}
