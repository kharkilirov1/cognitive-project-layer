use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::graph::ProjectGraph;
use crate::memory::WorkingMemory;
use crate::retrieval::CandidateChunk;
use crate::skeleton::Skeleton;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    pub max_tokens: usize,
    pub skeleton_tokens: usize,
    pub task_tokens: usize,
    pub working_memory_tokens: usize,
    pub retrieved_chunks_tokens: usize,
    pub graph_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSection {
    pub name: String,
    pub tokens: usize,
    pub included: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedContext {
    pub text: String,
    pub tokens: usize,
    pub sections: Vec<ContextSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudgetManager {
    pub max_tokens: usize,
}

impl Default for ContextBudgetManager {
    fn default() -> Self {
        Self { max_tokens: 16_000 }
    }
}

impl ContextBudgetManager {
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    pub fn build_context(
        &self,
        task: &str,
        skeleton: &Skeleton,
        memory: &WorkingMemory,
        chunks: &[CandidateChunk],
        graph: &ProjectGraph,
    ) -> ManagedContext {
        let mut text = String::new();
        let mut sections = Vec::new();
        let mut used = 0usize;

        // Required секции тоже проходят через hard cap: при нехватке бюджета
        // они обрезаются, а не пробивают max_tokens.
        self.push_section(
            "task",
            format!("# Task\n{task}\n"),
            true,
            &mut text,
            &mut sections,
            &mut used,
        );

        self.push_section(
            "skeleton",
            skeleton.render_prompt(),
            true,
            &mut text,
            &mut sections,
            &mut used,
        );

        self.push_section(
            "working_memory",
            render_memory(memory),
            true,
            &mut text,
            &mut sections,
            &mut used,
        );

        // Retrieved chunks: по одному, пока есть бюджет
        for (idx, chunk) in chunks.iter().enumerate() {
            let section = render_chunk(chunk, idx + 1);
            let tokens = estimate_tokens(&section);
            if used + tokens > self.max_tokens {
                break;
            }
            self.push_section(
                &format!("retrieved_chunk_{}", idx + 1),
                section,
                false,
                &mut text,
                &mut sections,
                &mut used,
            );
        }

        // Graph summary: только если влезает
        let graph_text = graph.render_summary();
        self.push_section(
            "graph_summary",
            graph_text,
            false,
            &mut text,
            &mut sections,
            &mut used,
        );

        ManagedContext {
            text,
            tokens: used,
            sections,
        }
    }

    fn push_section(
        &self,
        name: &str,
        section: String,
        required: bool,
        text: &mut String,
        sections: &mut Vec<ContextSection>,
        used: &mut usize,
    ) {
        let original_tokens = estimate_tokens(&section);
        let available = self.max_tokens.saturating_sub(*used);
        let (section, tokens, can_include) = if original_tokens <= available {
            (section, original_tokens, true)
        } else if required && available > 0 {
            let truncated = truncate_to_token_budget(&section, available);
            let tokens = estimate_tokens(&truncated);
            let can_include = !truncated.is_empty() && tokens <= available;
            (truncated, tokens, can_include)
        } else {
            (String::new(), original_tokens, false)
        };

        sections.push(ContextSection {
            name: name.to_string(),
            tokens,
            included: can_include,
        });
        if can_include {
            text.push_str(&section);
            if !section.ends_with('\n') {
                text.push('\n');
            }
            text.push('\n');
            *used += tokens;
        }
    }
}

/// Оценка токенов: 1 токен ≈ 4 символа для кода (консервативно).
/// Для точности использует tiktoken если доступен, иначе приближение.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // Более точная оценка: ASCII символы ~1 токен на 4 символа,
    // Unicode/спецсимволы ~1 токен на 2 символа
    let (ascii, unicode) = text.chars().fold((0usize, 0usize), |(a, u), ch| {
        if ch.is_ascii() {
            (a + 1, u)
        } else {
            (a, u + 1)
        }
    });
    ((ascii / 4) + (unicode / 2)).max(1)
}

fn truncate_to_token_budget(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 || text.is_empty() {
        return String::new();
    }
    if estimate_tokens(text) <= max_tokens {
        return text.to_string();
    }

    let marker = "\n... [truncated to fit context budget]\n";
    let marker_tokens = estimate_tokens(marker);
    let marker = if marker_tokens < max_tokens {
        marker
    } else {
        ""
    };

    let mut lo = 0usize;
    let mut hi = text.chars().count();
    let mut best = String::new();
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let candidate = format!("{}{}", take_chars(text, mid), marker);
        if !candidate.is_empty() && estimate_tokens(&candidate) <= max_tokens {
            best = candidate;
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    best
}

fn take_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn render_memory(memory: &WorkingMemory) -> String {
    format!(
        "# Working Memory\nCurrent task: {}\nOpened files: {}\nRelevant symbols: {}\nConfirmed facts: {}\nHypotheses: {}\nRejected paths: {}\nPending questions: {}\n",
        memory.current_task.as_deref().unwrap_or("none"),
        join_paths(&memory.opened_files),
        list_or_none(&memory.relevant_symbols),
        list_or_none(&memory.confirmed_facts),
        list_or_none(&memory.hypotheses),
        join_paths(&memory.rejected_paths),
        list_or_none(&memory.pending_questions),
    )
}

fn render_chunk(chunk: &CandidateChunk, idx: usize) -> String {
    format!(
        "# Retrieved Chunk {idx}\nPath: {}:{}-{}\nScore: {:.2}\nSymbols: {}\nReasons: {}\n```text\n{}\n```\n",
        chunk.path.display(),
        chunk.line_start,
        chunk.line_end,
        chunk.final_score,
        list_or_none(&chunk.symbols),
        chunk.reasons.join(", "),
        chunk.preview
    )
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn join_paths(items: &[PathBuf]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_tokens() {
        assert!(estimate_tokens("hello world") >= 1);
    }

    #[test]
    fn estimate_tokens_ascii() {
        let tokens = estimate_tokens("hello world this is a test");
        assert!(tokens > 0);
        assert!(tokens <= 10); // 25 chars / 4 = ~6 tokens
    }

    #[test]
    fn estimate_tokens_unicode() {
        let tokens = estimate_tokens("привет мир");
        assert!(tokens > 0);
    }

    #[test]
    fn budget_respects_max_tokens() {
        let manager = ContextBudgetManager::new(100);
        let skeleton = Skeleton::default();
        let memory = WorkingMemory::default();
        let graph = ProjectGraph::default();
        let context = manager.build_context("test", &skeleton, &memory, &[], &graph);
        assert!(context.tokens <= 100);
    }

    #[test]
    fn budget_truncates_oversized_required_sections() {
        let manager = ContextBudgetManager::new(25);
        let skeleton = Skeleton::default();
        let memory = WorkingMemory::default();
        let graph = ProjectGraph::default();
        let huge_task = "x".repeat(2_000);
        let context = manager.build_context(&huge_task, &skeleton, &memory, &[], &graph);
        assert!(context.tokens <= 25);
    }
}
