use serde::{Deserialize, Serialize};

use crate::indexer::LazyIndexer;
use crate::retrieval::RetrievalResult;
use crate::scanner::ProjectScan;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransparencyPanel {
    pub title: String,
}

impl Default for TransparencyPanel {
    fn default() -> Self {
        Self {
            title: "Context / Project Brain".to_string(),
        }
    }
}

impl TransparencyPanel {
    pub fn render(
        &self,
        scan: &ProjectScan,
        indexer: &LazyIndexer,
        result: Option<&RetrievalResult>,
    ) -> String {
        let width = 58usize;
        let mut lines = Vec::new();
        lines.push(format!(
            "┌─ {} {}",
            self.title,
            "─".repeat(width.saturating_sub(self.title.len() + 3))
        ));
        lines.push(row(width, &format!("Mode: {:?}", scan.recommended_mode)));
        lines.push(row(
            width,
            &format!(
                "Project: {} files / {} source",
                scan.total_files, scan.source_files
            ),
        ));
        lines.push(row(
            width,
            &format!(
                "Languages: {}",
                if scan.languages.is_empty() {
                    "none".to_string()
                } else {
                    scan.languages.join(", ")
                }
            ),
        ));
        lines.push(row(width, "Skeleton: ready"));
        lines.push(row(width, "Symbol Index: ready"));
        lines.push(row(width, &format!("Lazy Indexer: {:?}", indexer.state)));
        lines.push(row(
            width,
            &format!("RAG chunks: {}", indexer.vector_chunks),
        ));
        lines.push(row(width, &format!("Graph edges: {}", indexer.graph_edges)));

        if let Some(result) = result {
            lines.push(row(width, ""));
            lines.push(row(width, "Last retrieval:"));
            for chunk in result.chunks.iter().take(5) {
                lines.push(row(
                    width,
                    &format!(
                        "• {}:{} [{:.2}]",
                        chunk.path.display(),
                        chunk.line_start,
                        chunk.final_score
                    ),
                ));
            }
            lines.push(row(width, &format!("Confidence: {:.2}", result.confidence)));
            lines.push(row(width, &format!("Strategy: {:?}", result.strategy)));
            if !result.fallback_plan.is_empty() {
                lines.push(row(width, "Fallback: enabled"));
            }
        }

        lines.push(format!("└{}┘", "─".repeat(width)));
        lines.join("\n")
    }
}

fn row(width: usize, text: &str) -> String {
    let mut text = text.to_string();
    if text.chars().count() > width.saturating_sub(4) {
        text = text
            .chars()
            .take(width.saturating_sub(5))
            .collect::<String>()
            + "…";
    }
    let padding = width.saturating_sub(text.chars().count() + 2);
    format!("│ {text}{}│", " ".repeat(padding))
}
