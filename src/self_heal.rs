use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::budget::ContextBudgetManager;
use crate::doctor::{self, DoctorReport};
use crate::embedding::{EmbeddingBackend, EmbeddingConfig};
use crate::persistent_index::PersistentIndexRefreshResult;
use crate::persistent_vector::{
    PersistentVectorDb, PersistentVectorRefreshResult, refresh_and_save_default,
};
use crate::{
    CognitiveProjectLayer, DEFAULT_INDEX_REFRESH_LIMIT, refresh_or_rebuild_persistent_index,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SelfHealEmbeddingMode {
    Off,
    Existing,
    Ensure,
}

impl SelfHealEmbeddingMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Ok(Self::Off),
            "existing" | "present" | "refresh-existing" => Ok(Self::Existing),
            "ensure" | "on" | "1" | "true" | "yes" | "build" => Ok(Self::Ensure),
            other => anyhow::bail!(
                "unknown self-heal embeddings mode `{other}`; use off, existing, or ensure"
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SelfHealOptions {
    pub max_tokens: usize,
    pub max_incremental_files: usize,
    pub embeddings: SelfHealEmbeddingMode,
    pub embedding_config: Option<EmbeddingConfig>,
    pub allow_external_embeddings: bool,
    pub max_incremental_paths: usize,
}

impl Default for SelfHealOptions {
    fn default() -> Self {
        Self {
            max_tokens: ContextBudgetManager::default().max_tokens,
            max_incremental_files: DEFAULT_INDEX_REFRESH_LIMIT,
            embeddings: SelfHealEmbeddingMode::Existing,
            embedding_config: None,
            allow_external_embeddings: false,
            max_incremental_paths: DEFAULT_INDEX_REFRESH_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfHealReport {
    pub root: PathBuf,
    pub index: Option<PersistentIndexRefreshResult>,
    pub embeddings: Option<PersistentVectorRefreshResult>,
    pub doctor: Option<DoctorReport>,
    pub notes: Vec<String>,
    pub errors: Vec<String>,
}

impl SelfHealReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty() && self.doctor.as_ref().map(DoctorReport::ok).unwrap_or(false)
    }

    pub fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str("CPL self-heal\n");
        out.push_str(&format!("Root: {}\n", self.root.display()));
        out.push_str(&format!(
            "Overall: {}\n\n",
            if self.ok() { "ok" } else { "warnings/errors" }
        ));

        if let Some(index) = &self.index {
            out.push_str("Index:\n");
            out.push_str(&index.render_human());
            out.push_str("\n\n");
        }

        if let Some(embeddings) = &self.embeddings {
            out.push_str("Embeddings:\n");
            out.push_str(&embeddings.render_human());
            out.push_str("\n\n");
        }

        if !self.notes.is_empty() {
            out.push_str("Notes:\n");
            for note in &self.notes {
                out.push_str(&format!("- {note}\n"));
            }
            out.push('\n');
        }

        if !self.errors.is_empty() {
            out.push_str("Errors:\n");
            for error in &self.errors {
                out.push_str(&format!("- {error}\n"));
            }
            out.push('\n');
        }

        if let Some(doctor) = &self.doctor {
            out.push_str("Doctor after self-heal:\n");
            out.push_str(&doctor.render_human());
        }

        out
    }
}

pub fn self_heal(root: impl AsRef<Path>, options: SelfHealOptions) -> Result<SelfHealReport> {
    let root = root.as_ref().canonicalize()?;
    let mut notes = Vec::new();
    let mut errors = Vec::new();

    let index = match refresh_or_rebuild_persistent_index(
        &root,
        options.max_tokens,
        options.max_incremental_files,
    ) {
        Ok(result) => Some(result),
        Err(error) => {
            errors.push(format!("index refresh/rebuild failed: {error}"));
            None
        }
    };

    let embeddings = match refresh_embeddings_if_needed(&root, &options, &mut notes) {
        Ok(result) => result,
        Err(error) => {
            errors.push(format!("embedding refresh/build failed: {error}"));
            None
        }
    };

    let doctor = match doctor::run(&root) {
        Ok(report) => Some(report),
        Err(error) => {
            errors.push(format!("doctor failed after self-heal: {error}"));
            None
        }
    };

    Ok(SelfHealReport {
        root,
        index,
        embeddings,
        doctor,
        notes,
        errors,
    })
}

fn refresh_embeddings_if_needed(
    root: &Path,
    options: &SelfHealOptions,
    notes: &mut Vec<String>,
) -> Result<Option<PersistentVectorRefreshResult>> {
    match options.embeddings {
        SelfHealEmbeddingMode::Off => {
            notes.push("embedding self-heal disabled".to_string());
            return Ok(None);
        }
        SelfHealEmbeddingMode::Existing => {}
        SelfHealEmbeddingMode::Ensure => {}
    }

    let existing = PersistentVectorDb::load_default(root)?;
    if existing.is_none() && options.embeddings == SelfHealEmbeddingMode::Existing {
        notes.push("embedding vector DB is missing; skipped because mode=existing".to_string());
        return Ok(None);
    }

    let config = options
        .embedding_config
        .clone()
        .or_else(|| existing.as_ref().map(PersistentVectorDb::config))
        .unwrap_or_else(|| EmbeddingConfig::from_project_or_env(root));
    if !options.allow_external_embeddings && !is_local_embedding_config(&config) {
        notes.push(format!(
            "embedding self-heal skipped because backend {:?} may send code to an external service; rerun with explicit embedding backend if intended",
            config.backend
        ));
        return Ok(None);
    }
    let layer = CognitiveProjectLayer::initialize_with_budget(root, options.max_tokens)?;
    let (result, _db) = refresh_and_save_default(
        &layer.root,
        &layer.vector_store.chunks,
        config,
        options.max_incremental_paths,
    )?;
    Ok(Some(result))
}

fn is_local_embedding_config(config: &EmbeddingConfig) -> bool {
    match config.backend {
        EmbeddingBackend::LocalHash | EmbeddingBackend::Ollama => true,
        EmbeddingBackend::OpenAi => false,
        EmbeddingBackend::OpenAiCompatible => config
            .endpoint
            .as_deref()
            .map(|endpoint| {
                endpoint.contains("localhost")
                    || endpoint.contains("127.0.0.1")
                    || endpoint.contains("[::1]")
            })
            .unwrap_or(false),
    }
}

pub fn env_flag_enabled(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !(value == "0" || value == "false" || value == "off" || value == "no")
        })
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistent_index::PersistentIndex;
    use crate::scanner::ProjectScanner;

    #[test]
    fn self_heal_builds_missing_index_without_embeddings() {
        let root = temp_project("self_heal_builds_missing_index");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        std::fs::write(root.join("src").join("lib.rs"), "pub fn healed() {}\n").unwrap();

        let report = self_heal(
            &root,
            SelfHealOptions {
                embeddings: SelfHealEmbeddingMode::Off,
                ..SelfHealOptions::default()
            },
        )
        .unwrap();

        assert!(report.errors.is_empty(), "{}", report.render_human());
        assert!(report.index.is_some());
        let scan = ProjectScanner::default().scan(&root).unwrap();
        let freshness = PersistentIndex::freshness_default(&root, &scan).unwrap();
        assert!(freshness.fresh, "{}", freshness.render_human());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_embedding_modes() {
        assert_eq!(
            SelfHealEmbeddingMode::parse("off").unwrap(),
            SelfHealEmbeddingMode::Off
        );
        assert_eq!(
            SelfHealEmbeddingMode::parse("existing").unwrap(),
            SelfHealEmbeddingMode::Existing
        );
        assert_eq!(
            SelfHealEmbeddingMode::parse("ensure").unwrap(),
            SelfHealEmbeddingMode::Ensure
        );
        assert!(SelfHealEmbeddingMode::parse("bad").is_err());
    }

    #[test]
    fn external_embedding_configs_are_not_implicit_self_heal_targets() {
        let config = EmbeddingConfig::openai(Some("text-embedding-3-small".to_string()));
        assert!(!is_local_embedding_config(&config));
        assert!(is_local_embedding_config(&EmbeddingConfig::local_hash(64)));
        assert!(is_local_embedding_config(&EmbeddingConfig::ollama(
            Some("nomic-embed-text".to_string()),
            768
        )));
    }

    fn temp_project(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cpl-self-heal-{name}-{}", unique_suffix()))
    }

    fn unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }
}
