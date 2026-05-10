use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectConfig {
    pub ignore_paths: Vec<String>,
    pub embedding_backend: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_endpoint: Option<String>,
    pub embedding_dimensions: Option<usize>,
    pub context_max_tokens: Option<usize>,
    pub benchmark_recall10: Option<String>,
    pub benchmark_ndcg10: Option<String>,
    pub ui_default_tab: Option<String>,
}

impl ProjectConfig {
    pub fn load(root: &Path) -> Self {
        let path = root.join(".cpl").join("config.toml");
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        parse_project_config(&text)
    }
}

fn parse_project_config(text: &str) -> ProjectConfig {
    let mut config = ProjectConfig::default();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match (section.as_str(), key) {
            ("ignore", "paths") => config.ignore_paths = parse_string_array(value),
            ("embedding", "backend") => config.embedding_backend = parse_string(value),
            ("embedding", "model") => config.embedding_model = parse_string(value),
            ("embedding", "endpoint") => config.embedding_endpoint = parse_string(value),
            ("embedding", "dimensions") => config.embedding_dimensions = parse_usize(value),
            ("context", "max_tokens") => config.context_max_tokens = parse_usize(value),
            ("benchmarks", "recall10") => config.benchmark_recall10 = parse_string(value),
            ("benchmarks", "ndcg10") => config.benchmark_ndcg10 = parse_string(value),
            ("ui", "default_tab") => config.ui_default_tab = parse_string(value),
            _ => {}
        }
    }
    config
}

fn parse_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        Some(value[1..value.len() - 1].to_string())
    } else if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_usize(value: &str) -> Option<usize> {
    parse_string(value)?.parse().ok()
}

fn parse_string_array(value: &str) -> Vec<String> {
    let value = value.trim();
    let value = value
        .strip_prefix('[')
        .and_then(|item| item.strip_suffix(']'))
        .unwrap_or(value);
    value
        .split(',')
        .filter_map(parse_string)
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_config_sections() {
        let config = parse_project_config(
            r#"
            [ignore]
            paths = ["generated/", "vendor/"]

            [embedding]
            backend = "ollama"
            model = "nomic-embed-text"
            dimensions = 768

            [context]
            max_tokens = 64000

            [benchmarks]
            recall10 = "0.90"
            ndcg10 = "0.70"

            [ui]
            default_tab = "graph"
            "#,
        );
        assert_eq!(config.ignore_paths, vec!["generated/", "vendor/"]);
        assert_eq!(config.embedding_backend.as_deref(), Some("ollama"));
        assert_eq!(config.embedding_dimensions, Some(768));
        assert_eq!(config.context_max_tokens, Some(64000));
        assert_eq!(config.benchmark_recall10.as_deref(), Some("0.90"));
        assert_eq!(config.ui_default_tab.as_deref(), Some("graph"));
    }
}
