use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RetrievalStrategy {
    RagOnly,
    Hybrid,
    FallbackExplore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceReason {
    pub label: String,
    pub delta: f32,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceInput {
    pub top_score: f32,
    pub score_gap: f32,
    pub exact_symbol_match: bool,
    pub module_match: bool,
    pub graph_connected: bool,
    pub recent_match: bool,
    pub result_count: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfidenceProfile {
    FindSymbol,
    Architecture,
    BuildOrTestFailure,
    FixOrFeature,
    Generic,
}

#[derive(Debug, Clone, Copy)]
struct ConfidenceWeights {
    top_score: f32,
    score_gap: f32,
    exact_symbol: f32,
    module_match: f32,
    graph_connection: f32,
    recent_change: f32,
    result_count: f32,
    rag_threshold: f32,
    hybrid_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceReport {
    pub confidence: f32,
    pub strategy: RetrievalStrategy,
    pub reasons: Vec<ConfidenceReason>,
}

pub struct ConfidenceEngine;

impl ConfidenceEngine {
    /// Вычисляет confidence через логистическую функцию от взвешенной суммы
    /// независимых признаков. Все признаки нормализованы к [0,1].
    /// Пороги 0.80/0.50 выбраны эмпирически и могут быть откалиброваны.
    pub fn calculate(input: ConfidenceInput) -> ConfidenceReport {
        Self::calculate_with_profile(input, ConfidenceProfile::Generic)
    }

    pub fn calculate_with_profile(
        input: ConfidenceInput,
        profile: ConfidenceProfile,
    ) -> ConfidenceReport {
        let mut reasons = Vec::new();

        // Признак 1: top_score — уже взвешенная сумма, используем как есть
        let f_top_score = input.top_score.clamp(0.0, 1.0);

        // Признак 2: score_gap — насколько первый кандидат оторвался от второго
        let f_score_gap = (input.score_gap / 0.5).clamp(0.0, 1.0);

        // Признак 3: exact_symbol_match (бинарный)
        let f_exact_symbol = if input.exact_symbol_match { 1.0 } else { 0.0 };

        // Признак 4: module_match (бинарный)
        let f_module = if input.module_match { 1.0 } else { 0.0 };

        // Признак 5: graph_connected (бинарный)
        let f_graph = if input.graph_connected { 1.0 } else { 0.0 };

        // Признак 6: recent_match (бинарный)
        let f_recent = if input.recent_match { 1.0 } else { 0.0 };

        // Признак 7: достаточное количество результатов
        let f_count = (input.result_count as f32 / 12.0).clamp(0.0, 1.0);

        let profile_weights = weights_for_profile(profile);
        let weights = [
            (
                profile_weights.top_score,
                "top_score",
                f_top_score,
                "best merged candidate score",
            ),
            (
                profile_weights.score_gap,
                "score_gap",
                f_score_gap,
                "gap between top candidates",
            ),
            (
                profile_weights.exact_symbol,
                "exact_symbol",
                f_exact_symbol,
                "query has exact symbol match",
            ),
            (
                profile_weights.module_match,
                "module_match",
                f_module,
                "candidate path matches query module words",
            ),
            (
                profile_weights.graph_connection,
                "graph_connection",
                f_graph,
                "multiple candidates are in related paths",
            ),
            (
                profile_weights.recent_change,
                "recent_change",
                f_recent,
                "candidate intersects recent git changes",
            ),
            (
                profile_weights.result_count,
                "result_count",
                f_count,
                "sufficient number of candidates",
            ),
        ];

        let mut logit = 0.0;
        for (weight, label, value, _detail) in &weights {
            let contribution = weight * value;
            logit += contribution;
            reasons.push(ConfidenceReason {
                label: label.to_string(),
                delta: contribution,
                detail: format!(
                    "{} = {:.2} × {:.2} = {:.2}",
                    label, weight, value, contribution
                ),
            });
        }

        // Логистическая функция: confidence = 1 / (1 + exp(-6 * (logit - 0.5)))
        // Сдвиг 0.5 и крутизна 6 дают разумное распределение:
        // logit=0.0 -> conf≈0.05, logit=0.5 -> conf=0.50, logit=1.0 -> conf≈0.95
        let confidence = 1.0 / (1.0 + (-6.0 * (logit - 0.5)).exp());

        // Если нет результатов — confidence = 0
        let confidence = if input.result_count == 0 {
            0.0
        } else {
            confidence
        };

        reasons.push(ConfidenceReason {
            label: "profile".to_string(),
            delta: 0.0,
            detail: format!("{profile:?} intent profile"),
        });

        let strategy = if confidence >= profile_weights.rag_threshold {
            RetrievalStrategy::RagOnly
        } else if confidence >= profile_weights.hybrid_threshold {
            RetrievalStrategy::Hybrid
        } else {
            RetrievalStrategy::FallbackExplore
        };

        ConfidenceReport {
            confidence,
            strategy,
            reasons,
        }
    }
}

fn weights_for_profile(profile: ConfidenceProfile) -> ConfidenceWeights {
    match profile {
        ConfidenceProfile::FindSymbol => ConfidenceWeights {
            top_score: 0.18,
            score_gap: 0.08,
            exact_symbol: 0.42,
            module_match: 0.08,
            graph_connection: 0.12,
            recent_change: 0.02,
            result_count: 0.10,
            rag_threshold: 0.78,
            hybrid_threshold: 0.45,
        },
        ConfidenceProfile::Architecture => ConfidenceWeights {
            top_score: 0.18,
            score_gap: 0.06,
            exact_symbol: 0.08,
            module_match: 0.16,
            graph_connection: 0.30,
            recent_change: 0.07,
            result_count: 0.15,
            rag_threshold: 0.82,
            hybrid_threshold: 0.52,
        },
        ConfidenceProfile::BuildOrTestFailure => ConfidenceWeights {
            top_score: 0.18,
            score_gap: 0.08,
            exact_symbol: 0.10,
            module_match: 0.25,
            graph_connection: 0.20,
            recent_change: 0.09,
            result_count: 0.10,
            rag_threshold: 0.80,
            hybrid_threshold: 0.50,
        },
        ConfidenceProfile::FixOrFeature => ConfidenceWeights {
            top_score: 0.22,
            score_gap: 0.08,
            exact_symbol: 0.25,
            module_match: 0.15,
            graph_connection: 0.15,
            recent_change: 0.05,
            result_count: 0.10,
            rag_threshold: 0.80,
            hybrid_threshold: 0.50,
        },
        ConfidenceProfile::Generic => ConfidenceWeights {
            top_score: 0.25,
            score_gap: 0.10,
            exact_symbol: 0.25,
            module_match: 0.10,
            graph_connection: 0.10,
            recent_change: 0.05,
            result_count: 0.15,
            rag_threshold: 0.80,
            hybrid_threshold: 0.50,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_confidence_without_results() {
        let report = ConfidenceEngine::calculate(ConfidenceInput {
            top_score: 0.0,
            score_gap: 0.0,
            exact_symbol_match: false,
            module_match: false,
            graph_connected: false,
            recent_match: false,
            result_count: 0,
        });
        assert_eq!(report.strategy, RetrievalStrategy::FallbackExplore);
        assert_eq!(report.confidence, 0.0);
    }

    #[test]
    fn high_confidence_with_all_signals() {
        let report = ConfidenceEngine::calculate(ConfidenceInput {
            top_score: 0.9,
            score_gap: 0.4,
            exact_symbol_match: true,
            module_match: true,
            graph_connected: true,
            recent_match: true,
            result_count: 8,
        });
        assert!(report.confidence > 0.80);
        assert_eq!(report.strategy, RetrievalStrategy::RagOnly);
    }

    #[test]
    fn medium_confidence_with_partial_signals() {
        let report = ConfidenceEngine::calculate(ConfidenceInput {
            top_score: 0.5,
            score_gap: 0.1,
            exact_symbol_match: true,
            module_match: false,
            graph_connected: false,
            recent_match: false,
            result_count: 3,
        });
        // С такими сигналами confidence должен быть в среднем диапазоне
        assert!(report.confidence > 0.30);
        assert!(report.confidence < 0.90);
    }

    #[test]
    fn confidence_is_bounded() {
        let report = ConfidenceEngine::calculate(ConfidenceInput {
            top_score: 100.0,
            score_gap: 100.0,
            exact_symbol_match: true,
            module_match: true,
            graph_connected: true,
            recent_match: true,
            result_count: 100,
        });
        assert!(report.confidence <= 1.0);
        assert!(report.confidence >= 0.0);
    }

    #[test]
    fn confidence_is_monotonic() {
        let low = ConfidenceEngine::calculate(ConfidenceInput {
            top_score: 0.2,
            score_gap: 0.05,
            exact_symbol_match: false,
            module_match: false,
            graph_connected: false,
            recent_match: false,
            result_count: 1,
        });
        let high = ConfidenceEngine::calculate(ConfidenceInput {
            top_score: 0.9,
            score_gap: 0.4,
            exact_symbol_match: true,
            module_match: true,
            graph_connected: true,
            recent_match: true,
            result_count: 8,
        });
        assert!(high.confidence > low.confidence);
    }

    #[test]
    fn find_symbol_profile_rewards_exact_symbol_more_than_generic() {
        let input = ConfidenceInput {
            top_score: 0.3,
            score_gap: 0.0,
            exact_symbol_match: true,
            module_match: false,
            graph_connected: false,
            recent_match: false,
            result_count: 1,
        };
        let generic =
            ConfidenceEngine::calculate_with_profile(input.clone(), ConfidenceProfile::Generic);
        let find_symbol =
            ConfidenceEngine::calculate_with_profile(input, ConfidenceProfile::FindSymbol);
        assert!(find_symbol.confidence > generic.confidence);
    }
}
