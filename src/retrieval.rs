use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::confidence::{
    ConfidenceEngine, ConfidenceInput, ConfidenceProfile, ConfidenceReason, RetrievalStrategy,
};
use crate::graph::ProjectGraph;
use crate::memory::WorkingMemory;
use crate::persistent_vector::PersistentVectorDb;
use crate::references::ReferenceIndex;
use crate::scanner::{ProjectScan, is_text_candidate};
use crate::skeleton::Skeleton;
use crate::symbols::{SymbolIndex, SymbolLocation};
use crate::tools::{FallbackTools, FileCache, validate_path};
use crate::vector::VectorStore;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum QueryIntent {
    FindSymbol,
    ExplainCode,
    FixBug,
    AddFeature,
    Refactor,
    BuildError,
    TestFailure,
    ArchitectureQuestion,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryAnalysis {
    pub raw: String,
    pub intent: QueryIntent,
    pub symbols: Vec<String>,
    pub terms: Vec<String>,
    pub module_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateChunk {
    pub path: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub preview: String,
    pub symbols: Vec<String>,
    pub symbol_score: f32,
    pub lexical_score: f32,
    pub semantic_score: f32,
    pub graph_score: f32,
    pub recency_score: f32,
    pub final_score: f32,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    pub query: QueryAnalysis,
    pub chunks: Vec<CandidateChunk>,
    pub confidence: f32,
    pub strategy: RetrievalStrategy,
    pub reasons: Vec<ConfidenceReason>,
    pub skeleton_prompt: String,
    pub fallback_plan: Vec<String>,
}

impl RetrievalResult {
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str("Cognitive retrieval result\n");
        out.push_str(&format!("Intent: {:?}\n", self.query.intent));
        out.push_str(&format!(
            "Confidence: {:.2}; strategy: {:?}\n",
            self.confidence, self.strategy
        ));
        out.push_str(&format!(
            "Symbols: {}; terms: {}\n",
            list_or_none(&self.query.symbols),
            list_or_none(&self.query.terms)
        ));
        out.push_str("\nCandidates:\n");
        if self.chunks.is_empty() {
            out.push_str("- none\n");
        }
        for chunk in &self.chunks {
            out.push_str(&format!(
                "- [{:.2}] {}:{}-{} symbols={} reasons={}\n",
                chunk.final_score,
                chunk.path.display(),
                chunk.line_start,
                chunk.line_end,
                list_or_none(&chunk.symbols),
                chunk.reasons.join(", ")
            ));
            if !chunk.preview.trim().is_empty() {
                out.push_str(&indent(&chunk.preview, "    "));
                out.push('\n');
            }
        }
        out.push_str("\nConfidence reasons:\n");
        for reason in &self.reasons {
            out.push_str(&format!(
                "- {:+.2} {} — {}\n",
                reason.delta, reason.label, reason.detail
            ));
        }
        if !self.fallback_plan.is_empty() {
            out.push_str("\nFallback plan:\n");
            for item in &self.fallback_plan {
                out.push_str(&format!("- {item}\n"));
            }
        }
        out
    }
}

pub struct HybridRetriever;

pub struct RetrieverResources<'a> {
    pub root: &'a Path,
    pub scan: &'a ProjectScan,
    pub skeleton: &'a Skeleton,
    pub symbols: &'a SymbolIndex,
    pub graph: &'a ProjectGraph,
    pub references: &'a ReferenceIndex,
    pub vector_store: &'a VectorStore,
    pub persistent_vector_db: Option<&'a PersistentVectorDb>,
    pub memory: &'a WorkingMemory,
}

impl HybridRetriever {
    pub fn retrieve(resources: RetrieverResources<'_>, query: &str) -> Result<RetrievalResult> {
        let root = resources.root;
        let scan = resources.scan;
        let skeleton = resources.skeleton;
        let symbols = resources.symbols;
        let graph = resources.graph;
        let references = resources.references;
        let vector_store = resources.vector_store;
        let persistent_vector_db = resources.persistent_vector_db;
        let memory = resources.memory;
        let analysis = analyze_query(query)?;
        let mut candidates = BTreeMap::<(PathBuf, usize), CandidateChunk>::new();

        // 1. Symbol lookup
        let symbol_hits = symbol_lookup(symbols, &analysis);
        for symbol in &symbol_hits {
            let rel = relative_path(root, &symbol.path);
            let preview = excerpt_abs(root, &rel, symbol.line_start, 4).unwrap_or_default();
            let key = (rel.clone(), symbol.line_start);
            let entry = candidates.entry(key).or_insert_with(|| CandidateChunk {
                path: rel.clone(),
                line_start: symbol.line_start,
                line_end: symbol.line_end,
                preview,
                symbols: vec![symbol.name.clone()],
                symbol_score: 0.0,
                lexical_score: 0.0,
                semantic_score: 0.0,
                graph_score: 0.0,
                recency_score: 0.0,
                final_score: 0.0,
                reasons: vec!["symbol_lookup".to_string()],
            });
            entry.symbol_score = entry.symbol_score.max(1.0);
            if !entry.symbols.contains(&symbol.name) {
                entry.symbols.push(symbol.name.clone());
            }
        }

        // 2. References/usages index
        let reference_queries = reference_query_names(&analysis);
        for reference in references.find_any(reference_queries.iter().map(String::as_str), 12) {
            let preview =
                FallbackTools::open_file_excerpt(root, &reference.path, reference.line_number, 2)
                    .unwrap_or_else(|_| reference.snippet.clone());
            let key = (reference.path.clone(), reference.line_number);
            let entry = candidates.entry(key).or_insert_with(|| CandidateChunk {
                path: reference.path.clone(),
                line_start: reference.line_number,
                line_end: reference.line_number,
                preview,
                symbols: vec![reference.symbol_name.clone()],
                symbol_score: 0.0,
                lexical_score: 0.0,
                semantic_score: 0.0,
                graph_score: 0.0,
                recency_score: 0.0,
                final_score: 0.0,
                reasons: Vec::new(),
            });
            entry.symbol_score = entry.symbol_score.max(0.75);
            entry.graph_score = entry.graph_score.max(0.35);
            if !entry.symbols.contains(&reference.symbol_name) {
                entry.symbols.push(reference.symbol_name);
            }
            if !entry
                .reasons
                .iter()
                .any(|reason| reason == "reference_index")
            {
                entry.reasons.push("reference_index".to_string());
            }
        }

        // 3. Grep
        for term in analysis.terms.iter().take(8) {
            for hit in FallbackTools::grep(root, term, 40)? {
                let preview = FallbackTools::open_file_excerpt(root, &hit.path, hit.line_number, 2)
                    .unwrap_or_else(|_| hit.line.clone());
                let key = (hit.path.clone(), hit.line_number);
                let entry = candidates.entry(key).or_insert_with(|| CandidateChunk {
                    path: hit.path.clone(),
                    line_start: hit.line_number,
                    line_end: hit.line_number,
                    preview,
                    symbols: Vec::new(),
                    symbol_score: 0.0,
                    lexical_score: 0.0,
                    semantic_score: 0.0,
                    graph_score: 0.0,
                    recency_score: 0.0,
                    final_score: 0.0,
                    reasons: Vec::new(),
                });
                entry.lexical_score = (entry.lexical_score + 0.20).min(1.0);
                if !entry.reasons.iter().any(|reason| reason == "grep") {
                    entry.reasons.push("grep".to_string());
                }
            }
        }

        // 4. Vector search (TF-IDF)
        for hit in vector_store.search(query, 30) {
            let key = (hit.chunk.path.clone(), hit.chunk.line_start);
            let entry = candidates.entry(key).or_insert_with(|| CandidateChunk {
                path: hit.chunk.path.clone(),
                line_start: hit.chunk.line_start,
                line_end: hit.chunk.line_end,
                preview: hit.chunk.source.clone(),
                symbols: hit.chunk.symbols.clone(),
                symbol_score: 0.0,
                lexical_score: 0.0,
                semantic_score: 0.0,
                graph_score: 0.0,
                recency_score: 0.0,
                final_score: 0.0,
                reasons: Vec::new(),
            });
            entry.semantic_score = entry.semantic_score.max(hit.score.min(1.0));
            for symbol in hit.chunk.symbols {
                if !entry.symbols.contains(&symbol) {
                    entry.symbols.push(symbol);
                }
            }
            if !entry.reasons.iter().any(|reason| reason == "vector_search") {
                entry.reasons.push("vector_search".to_string());
            }
        }

        // 5. Persistent vector DB (dense embeddings)
        if let Some(db) = persistent_vector_db
            && let Ok(client) = crate::embedding::EmbeddingClient::new(db.config()).embed_one(query)
        {
            for hit in db.search_vector(&client, 30) {
                let key = (hit.chunk.path.clone(), hit.chunk.line_start);
                let entry = candidates.entry(key).or_insert_with(|| CandidateChunk {
                    path: hit.chunk.path.clone(),
                    line_start: hit.chunk.line_start,
                    line_end: hit.chunk.line_end,
                    preview: hit.chunk.source.clone(),
                    symbols: hit.chunk.symbols.clone(),
                    symbol_score: 0.0,
                    lexical_score: 0.0,
                    semantic_score: 0.0,
                    graph_score: 0.0,
                    recency_score: 0.0,
                    final_score: 0.0,
                    reasons: Vec::new(),
                });
                entry.semantic_score = entry.semantic_score.max(hit.score.min(1.0));
                for symbol in hit.chunk.symbols {
                    if !entry.symbols.contains(&symbol) {
                        entry.symbols.push(symbol);
                    }
                }
                if !entry
                    .reasons
                    .iter()
                    .any(|reason| reason == "persistent_vector_db")
                {
                    entry.reasons.push("persistent_vector_db".to_string());
                }
            }
        }

        // 6. Skeleton context paths
        for rel_path in skeleton_context_paths(skeleton, &analysis) {
            let abs = validate_path(root, &rel_path).ok();
            let abs = match abs {
                Some(p) if p.exists() && is_text_candidate(&p) => p,
                _ => continue,
            };
            let preview = FileCache::read_to_string(&abs)
                .ok()
                .map(|source| source.lines().take(30).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            let key = (rel_path.clone(), 1);
            let entry = candidates.entry(key).or_insert_with(|| CandidateChunk {
                path: rel_path.clone(),
                line_start: 1,
                line_end: 30,
                preview,
                symbols: Vec::new(),
                symbol_score: 0.0,
                lexical_score: 0.0,
                semantic_score: 0.0,
                graph_score: 0.0,
                recency_score: 0.0,
                final_score: 0.0,
                reasons: vec!["skeleton_context".to_string()],
            });
            entry.graph_score = entry.graph_score.max(0.35);
        }

        // 7. Graph expansion
        let mut graph_seeds = candidates
            .values()
            .filter(|chunk| chunk.symbol_score > 0.0 || chunk.semantic_score > 0.10)
            .map(|chunk| chunk.path.clone())
            .collect::<Vec<_>>();
        graph_seeds.extend(graph.related_to_symbol_paths(&analysis.symbols, 2));
        for rel_path in graph.expand_files(&graph_seeds, 2) {
            add_file_candidate(root, &mut candidates, &rel_path, 1, "graph_expansion", 0.55);
        }

        // 8. Boost from working memory
        boost_related_candidates(&mut candidates, memory);

        // 9. Score
        score_candidates(&mut candidates, scan, &analysis);

        let mut chunks = candidates.into_values().collect::<Vec<_>>();
        chunks.sort_by(|left, right| {
            right
                .final_score
                .partial_cmp(&left.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line_start.cmp(&right.line_start))
        });
        chunks.truncate(12);

        let top_score = chunks.first().map(|chunk| chunk.final_score).unwrap_or(0.0);
        let second_score = chunks.get(1).map(|chunk| chunk.final_score).unwrap_or(0.0);
        let exact_symbol_match = analysis
            .symbols
            .iter()
            .any(|query_symbol| symbols.by_name.contains_key(query_symbol));
        let module_match = chunks
            .iter()
            .any(|chunk| path_matches_modules(&chunk.path, &analysis.module_hints));
        let graph_connected = has_related_paths(&chunks);
        let recent_match = chunks.iter().any(|chunk| {
            scan.recent_changed_files
                .iter()
                .any(|recent| same_rel_path(recent, &chunk.path))
        });
        let confidence = ConfidenceEngine::calculate_with_profile(
            ConfidenceInput {
                top_score,
                score_gap: top_score - second_score,
                exact_symbol_match,
                module_match,
                graph_connected,
                recent_match,
                result_count: chunks.len(),
            },
            confidence_profile_for_intent(&analysis.intent),
        );

        let fallback_plan = if confidence.strategy == RetrievalStrategy::FallbackExplore {
            vec![
                "Use file_tree to re-orient on project structure".to_string(),
                format!("Run grep for: {}", analysis.terms.join(", ")),
                "Use symbol_lookup for exact identifiers if the query contains code names"
                    .to_string(),
                "Open candidate files manually before answering or editing".to_string(),
            ]
        } else if confidence.strategy == RetrievalStrategy::Hybrid {
            vec![
                "Use retrieved chunks plus grep verification before editing".to_string(),
                "Expand to neighboring imports/tests when candidate file is confirmed".to_string(),
            ]
        } else {
            Vec::new()
        };

        Ok(RetrievalResult {
            query: analysis,
            chunks,
            confidence: confidence.confidence,
            strategy: confidence.strategy,
            reasons: confidence.reasons,
            skeleton_prompt: skeleton.render_prompt(),
            fallback_plan,
        })
    }
}

fn analyze_query(query: &str) -> Result<QueryAnalysis> {
    let lower = query.to_lowercase();
    let intent = if contains_any(
        &lower,
        &[
            "build",
            "сбор",
            "линков",
            "compile",
            "hilog",
            "cmake",
            "hvigor",
        ],
    ) {
        QueryIntent::BuildError
    } else if contains_any(
        &lower,
        &["где", "where", "используется", "объявлена", "find"],
    ) {
        QueryIntent::FindSymbol
    } else if contains_any(
        &lower,
        &["почему", "bug", "баг", "не работает", "fix", "ошибка"],
    ) {
        QueryIntent::FixBug
    } else if contains_any(&lower, &["добавь", "add", "feature", "реализуй"]) {
        QueryIntent::AddFeature
    } else if contains_any(&lower, &["refactor", "рефактор"]) {
        QueryIntent::Refactor
    } else if contains_any(&lower, &["test", "тест", "assert", "failed"]) {
        QueryIntent::TestFailure
    } else if contains_any(&lower, &["архитект", "architecture", "объясни", "explain"])
    {
        QueryIntent::ArchitectureQuestion
    } else {
        QueryIntent::Unknown
    };

    let identifier_re = Regex::new(r"[A-Za-z_$][A-Za-z0-9_$]{2,}")?;
    let mut symbols = BTreeSet::new();
    let mut terms = BTreeSet::new();
    for found in identifier_re.find_iter(query) {
        let item = found.as_str();
        if item
            .chars()
            .any(|ch| ch.is_ascii_uppercase() || ch == '_' || ch == '$')
        {
            symbols.insert(item.to_string());
        }
        if !STOP_WORDS.contains(&item.to_lowercase().as_str()) {
            terms.insert(item.to_string());
        }
    }

    for word in lower
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .filter(|word| word.chars().count() >= 3)
    {
        if !STOP_WORDS.contains(&word) {
            terms.insert(word.to_string());
        }
    }

    let module_hints = terms
        .iter()
        .filter(|term| {
            matches!(
                term.as_str(),
                "auth"
                    | "login"
                    | "jwt"
                    | "db"
                    | "api"
                    | "route"
                    | "ui"
                    | "chat"
                    | "native"
                    | "napi"
                    | "hilog"
                    | "test"
                    | "store"
                    | "selector"
                    | "page"
                    | "component"
            )
        })
        .cloned()
        .collect();

    Ok(QueryAnalysis {
        raw: query.to_string(),
        intent,
        symbols: symbols.into_iter().collect(),
        terms: terms.into_iter().collect(),
        module_hints,
    })
}

fn symbol_lookup(symbols: &SymbolIndex, analysis: &QueryAnalysis) -> Vec<SymbolLocation> {
    let mut hits = Vec::new();
    let mut seen = BTreeSet::new();
    for symbol_name in &analysis.symbols {
        for symbol in symbols.find(symbol_name).into_iter().take(8) {
            let key = (symbol.path.clone(), symbol.line_start, symbol.name.clone());
            if seen.insert(key) {
                hits.push(symbol);
            }
        }
    }
    if hits.is_empty() && analysis.intent == QueryIntent::FindSymbol {
        for term in &analysis.terms {
            for symbol in symbols.find(term).into_iter().take(5) {
                let key = (symbol.path.clone(), symbol.line_start, symbol.name.clone());
                if seen.insert(key) {
                    hits.push(symbol);
                }
            }
        }
    }
    hits
}

fn reference_query_names(analysis: &QueryAnalysis) -> Vec<String> {
    let mut names = analysis.symbols.iter().cloned().collect::<BTreeSet<_>>();
    if matches!(
        analysis.intent,
        QueryIntent::FindSymbol | QueryIntent::FixBug
    ) {
        names.extend(analysis.terms.iter().cloned());
    }
    names.into_iter().collect()
}

fn confidence_profile_for_intent(intent: &QueryIntent) -> ConfidenceProfile {
    match intent {
        QueryIntent::FindSymbol => ConfidenceProfile::FindSymbol,
        QueryIntent::ArchitectureQuestion => ConfidenceProfile::Architecture,
        QueryIntent::BuildError | QueryIntent::TestFailure => ConfidenceProfile::BuildOrTestFailure,
        QueryIntent::FixBug | QueryIntent::AddFeature | QueryIntent::Refactor => {
            ConfidenceProfile::FixOrFeature
        }
        QueryIntent::ExplainCode | QueryIntent::Unknown => ConfidenceProfile::Generic,
    }
}

fn skeleton_context_paths(skeleton: &Skeleton, analysis: &QueryAnalysis) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    if matches!(
        analysis.intent,
        QueryIntent::BuildError | QueryIntent::ArchitectureQuestion | QueryIntent::Unknown
    ) {
        for entry in &skeleton.entry_points {
            paths.insert(entry.path.clone());
        }
        for config in &skeleton.configs {
            paths.insert(config.path.clone());
        }
    }
    for module in &skeleton.modules {
        if path_matches_modules(&module.path, &analysis.module_hints) {
            for file in &module.key_files {
                paths.insert(file.clone());
            }
        }
    }
    paths.into_iter().collect()
}

fn boost_related_candidates(
    candidates: &mut BTreeMap<(PathBuf, usize), CandidateChunk>,
    memory: &WorkingMemory,
) {
    for chunk in candidates.values_mut() {
        if memory
            .opened_files
            .iter()
            .any(|path| same_rel_path(path, &chunk.path))
        {
            chunk.graph_score = chunk.graph_score.max(0.25);
            chunk.reasons.push("working_memory".to_string());
        }
    }
}

fn add_file_candidate(
    root: &Path,
    candidates: &mut BTreeMap<(PathBuf, usize), CandidateChunk>,
    rel_path: &Path,
    line_start: usize,
    reason: &str,
    graph_score: f32,
) {
    let abs = validate_path(root, rel_path).ok();
    let abs = match abs {
        Some(p) if p.exists() && is_text_candidate(&p) => p,
        _ => return,
    };
    let preview = FileCache::read_to_string(&abs)
        .ok()
        .map(|source| {
            source
                .lines()
                .skip(line_start.saturating_sub(1))
                .take(40)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let key = (rel_path.to_path_buf(), line_start);
    let entry = candidates.entry(key).or_insert_with(|| CandidateChunk {
        path: rel_path.to_path_buf(),
        line_start,
        line_end: line_start + 39,
        preview,
        symbols: Vec::new(),
        symbol_score: 0.0,
        lexical_score: 0.0,
        semantic_score: 0.0,
        graph_score: 0.0,
        recency_score: 0.0,
        final_score: 0.0,
        reasons: Vec::new(),
    });
    entry.graph_score = entry.graph_score.max(graph_score);
    if !entry.reasons.iter().any(|item| item == reason) {
        entry.reasons.push(reason.to_string());
    }
}

fn score_candidates(
    candidates: &mut BTreeMap<(PathBuf, usize), CandidateChunk>,
    scan: &ProjectScan,
    analysis: &QueryAnalysis,
) {
    for chunk in candidates.values_mut() {
        let path_text = chunk.path.to_string_lossy().to_lowercase();
        let preview_lower = chunk.preview.to_lowercase();
        let term_hits = analysis
            .terms
            .iter()
            .filter(|term| {
                path_text.contains(&term.to_lowercase())
                    || preview_lower.contains(&term.to_lowercase())
            })
            .count();
        if !analysis.terms.is_empty() {
            let lexical_semantic = (term_hits as f32 / analysis.terms.len() as f32).clamp(0.0, 1.0);
            chunk.semantic_score = chunk.semantic_score.max(lexical_semantic);
        }
        if path_matches_modules(&chunk.path, &analysis.module_hints) {
            chunk.lexical_score = (chunk.lexical_score + 0.25).min(1.0);
            chunk.reasons.push("module_hint".to_string());
        }
        if scan
            .recent_changed_files
            .iter()
            .any(|recent| same_rel_path(recent, &chunk.path))
        {
            chunk.recency_score = 1.0;
            chunk.reasons.push("recent_change".to_string());
        }

        let (symbol_w, lexical_w, semantic_w, graph_w, recency_w) = match analysis.intent {
            QueryIntent::FindSymbol => (0.50, 0.25, 0.10, 0.10, 0.05),
            QueryIntent::ArchitectureQuestion => (0.15, 0.15, 0.25, 0.35, 0.10),
            QueryIntent::BuildError => (0.10, 0.35, 0.20, 0.25, 0.10),
            QueryIntent::FixBug | QueryIntent::TestFailure => (0.30, 0.25, 0.20, 0.15, 0.10),
            _ => (0.30, 0.25, 0.20, 0.15, 0.10),
        };
        chunk.final_score = symbol_w * chunk.symbol_score
            + lexical_w * chunk.lexical_score
            + semantic_w * chunk.semantic_score
            + graph_w * chunk.graph_score
            + recency_w * chunk.recency_score;
    }
}

fn excerpt_abs(root: &Path, rel: &Path, line_start: usize, context: usize) -> Result<String> {
    let abs = validate_path(root, rel)?;
    let source = FileCache::read_to_string(&abs)?;
    let start = line_start.saturating_sub(context).max(1);
    let end = line_start + context;
    let mut out = String::new();
    for (idx, line) in source.lines().enumerate() {
        let line_no = idx + 1;
        if line_no >= start && line_no <= end {
            out.push_str(&format!("{line_no:>5}: {line}\n"));
        }
    }
    Ok(out)
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn path_matches_modules(path: &Path, hints: &[String]) -> bool {
    if hints.is_empty() {
        return false;
    }
    let text = path.to_string_lossy().to_lowercase();
    hints.iter().any(|hint| text.contains(&hint.to_lowercase()))
}

fn has_related_paths(chunks: &[CandidateChunk]) -> bool {
    let mut dirs = BTreeSet::new();
    for chunk in chunks {
        if let Some(parent) = chunk.path.parent() {
            dirs.insert(parent.to_path_buf());
        }
    }
    !dirs.is_empty() && chunks.len() >= 2 && dirs.len() <= chunks.len()
}

fn same_rel_path(left: &Path, right: &Path) -> bool {
    let left = left.to_string_lossy().replace('\\', "/");
    let right = right.to_string_lossy().replace('\\', "/");
    left == right || left.ends_with(&right) || right.ends_with(&left)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

const STOP_WORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "this",
    "that",
    "что",
    "как",
    "где",
    "или",
    "для",
    "при",
    "после",
    "почему",
    "работает",
    "нужно",
    "надо",
    "есть",
    "from",
    "into",
    "about",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_build_error_intent_for_hilog() {
        let analysis = analyze_query("Почему сборка падает на hilog?").unwrap();
        assert_eq!(analysis.intent, QueryIntent::BuildError);
        assert!(analysis.terms.iter().any(|term| term == "hilog"));
    }

    #[test]
    fn reference_query_names_include_terms_for_find_symbol() {
        let analysis = analyze_query("Где используется validate_token?").unwrap();
        let names = reference_query_names(&analysis);
        assert!(names.iter().any(|name| name == "validate_token"));
    }
}
