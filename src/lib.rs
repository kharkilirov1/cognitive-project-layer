pub mod ast;
pub mod background;
pub mod budget;
pub mod chunk;
pub mod confidence;
pub mod doctor;
pub mod embedding;
pub mod graph;
pub mod http_server;
pub mod indexer;
pub mod mcp_server;
pub mod memory;
pub mod persistent_index;
pub mod persistent_vector;
pub mod qdrant;
pub mod references;
pub mod retrieval;
pub mod scanner;
pub mod skeleton;
pub mod symbols;
pub mod tools;
pub mod transparency;
pub mod vector;
pub mod watcher;

use std::path::{Path, PathBuf};

use anyhow::Result;
use budget::{ContextBudgetManager, ManagedContext};
use chunk::{RichChunk, chunk_file};
use graph::ProjectGraph;
use indexer::LazyIndexer;
use memory::WorkingMemory;
use persistent_index::{
    PersistentIndex, PersistentIndexFreshness, PersistentIndexRefreshMode,
    PersistentIndexRefreshResult,
};
use persistent_vector::PersistentVectorDb;
use references::ReferenceIndex;
use retrieval::{HybridRetriever, RetrievalResult, RetrieverResources};
use scanner::{ProjectScan, ProjectScanner, detect_language, is_config_file, is_entry_candidate};
use serde::{Deserialize, Serialize};
use skeleton::Skeleton;
use symbols::SymbolIndex;
use tools::{FileCache, validate_path};
use transparency::TransparencyPanel;
use vector::VectorStore;

pub const DEFAULT_INDEX_REFRESH_LIMIT: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveProjectLayer {
    pub root: PathBuf,
    pub scan: ProjectScan,
    pub skeleton: Skeleton,
    pub symbols: SymbolIndex,
    pub graph: ProjectGraph,
    pub references: ReferenceIndex,
    pub indexer: LazyIndexer,
    pub vector_store: VectorStore,
    pub persistent_vector_db: Option<PersistentVectorDb>,
    pub memory: WorkingMemory,
    pub budget: ContextBudgetManager,
}

pub fn refresh_or_rebuild_persistent_index(
    root: impl AsRef<Path>,
    max_tokens: usize,
    max_incremental_files: usize,
) -> Result<PersistentIndexRefreshResult> {
    let root = normalize_root(root.as_ref())?;
    let scan = ProjectScanner::default().scan(&root)?;
    let incremental_error =
        match PersistentIndex::refresh_incremental_default(&root, &scan, max_incremental_files) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => None,
            Err(error) => Some(error.to_string()),
        };

    let mut freshness_before = match PersistentIndex::freshness_default(&root, &scan) {
        Ok(freshness) => freshness,
        Err(error) => PersistentIndexFreshness {
            path: PersistentIndex::default_path(&root),
            exists: PersistentIndex::default_path(&root).exists(),
            fresh: false,
            schema_version: None,
            reason: format!("index freshness check failed: {error}"),
            current_files: scan.source_paths.len()
                + scan.config_files.len()
                + scan.entry_candidates.len(),
            indexed_files: 0,
            changed_files: Vec::new(),
            missing_files: Vec::new(),
            extra_files: Vec::new(),
        },
    };
    if let Some(error) = incremental_error {
        freshness_before.reason = format!("incremental refresh failed: {error}");
    }
    let layer = CognitiveProjectLayer::initialize_with_budget(&root, max_tokens)?;
    let (summary, path) = PersistentIndex::build_default(
        &layer.root,
        &layer.scan,
        &layer.symbols,
        &layer.references,
        &layer.graph,
        &layer.vector_store.chunks,
    )?;
    let freshness_after = PersistentIndex::freshness_default(&layer.root, &layer.scan)?;

    Ok(PersistentIndexRefreshResult {
        mode: PersistentIndexRefreshMode::Rebuilt,
        path,
        touched_files: freshness_change_count(&freshness_before),
        summary,
        freshness_before,
        freshness_after,
    })
}

fn freshness_change_count(freshness: &PersistentIndexFreshness) -> usize {
    freshness.changed_files.len() + freshness.missing_files.len() + freshness.extra_files.len()
}

impl CognitiveProjectLayer {
    pub fn initialize(root: impl AsRef<Path>) -> Result<Self> {
        let root = normalize_root(root.as_ref())?;
        let scanner = ProjectScanner::default();
        let scan = scanner.scan(&root)?;
        let snapshot = PersistentIndex::load_snapshot_default(&root, &scan).unwrap_or_default();
        let (symbols, graph, references, chunks) = if let Some(snapshot) = snapshot {
            (
                snapshot.symbols,
                snapshot.graph,
                snapshot.references,
                snapshot.chunks,
            )
        } else {
            let symbols = SymbolIndex::build(&scan.source_paths)?;
            let graph = ProjectGraph::build(&root, &scan, &symbols)?;
            let references = ReferenceIndex::build(&root, &scan, &symbols)?;
            let chunks = RichChunk::chunk_project(&root, &scan, &symbols)?;
            (symbols, graph, references, chunks)
        };
        let skeleton = Skeleton::build(&scan, &symbols);
        let vector_store = VectorStore::build(chunks);
        let persistent_vector_db = PersistentVectorDb::load_default(&root)?;
        let mut indexer = LazyIndexer::skeleton(&scan);
        indexer.mark_hot(
            &graph,
            persistent_vector_db
                .as_ref()
                .map(|db| db.records.len())
                .unwrap_or_else(|| vector_store.len()),
        );

        Ok(Self {
            root,
            scan,
            skeleton,
            symbols,
            graph,
            references,
            indexer,
            vector_store,
            persistent_vector_db,
            memory: WorkingMemory::default(),
            budget: ContextBudgetManager::default(),
        })
    }

    pub fn initialize_with_budget(root: impl AsRef<Path>, max_tokens: usize) -> Result<Self> {
        let mut layer = Self::initialize(root)?;
        layer.set_context_budget(max_tokens);
        Ok(layer)
    }

    pub fn set_context_budget(&mut self, max_tokens: usize) {
        self.budget = ContextBudgetManager::new(max_tokens);
    }

    pub fn retrieve(&mut self, query: &str) -> Result<RetrievalResult> {
        let result = HybridRetriever::retrieve(
            RetrieverResources {
                root: &self.root,
                scan: &self.scan,
                skeleton: &self.skeleton,
                symbols: &self.symbols,
                graph: &self.graph,
                references: &self.references,
                vector_store: &self.vector_store,
                persistent_vector_db: self.persistent_vector_db.as_ref(),
                memory: &self.memory,
            },
            query,
        )?;

        self.memory.remember_retrieval(query, &result);
        for chunk in &result.chunks {
            self.indexer.mark_touched(chunk.path.clone(), &self.graph);
        }
        Ok(result)
    }

    pub fn build_context(&self, task: &str, retrieval: &RetrievalResult) -> ManagedContext {
        self.budget.build_context(
            task,
            &self.skeleton,
            &self.memory,
            &retrieval.chunks,
            &self.graph,
        )
    }

    pub fn transparency_panel(&self, retrieval: Option<&RetrievalResult>) -> String {
        TransparencyPanel::default().render(&self.scan, &self.indexer, retrieval)
    }

    /// Инкрементальное обновление при сохранении файла.
    /// Перестраивает только то, что изменилось:
    /// - символы для изменённого файла
    /// - граф (только edges для изменённого файла)
    /// - чанки для изменённого файла
    /// - векторный индекс (замена chunks только для изменённого файла)
    pub fn on_file_save(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let abs = validate_path(&self.root, path.as_ref())?;
        let rel = abs.strip_prefix(&self.root).unwrap_or(&abs).to_path_buf();

        // 1. Инвалидируем кэш файла
        FileCache::invalidate(&abs);

        // 2. Обновляем scan metadata для изменённого файла без полного walk.
        self.refresh_scan_for_saved_file(&abs, &rel);

        // 3. Обновляем символы и skeleton без чтения всего проекта.
        self.symbols.refresh_file(&abs)?;
        self.references
            .refresh_file(&self.root, &abs, &self.symbols)?;
        self.skeleton = Skeleton::build(&self.scan, &self.symbols);

        // 4. Обновляем только поверхность графа, связанную с этим файлом.
        self.graph
            .refresh_file(&self.root, &self.scan, &self.symbols, &abs)?;

        // 5. Перестраиваем только чанки изменённого файла.
        let updated_chunks = chunk_file(&self.root, &abs, &self.symbols)?;
        self.vector_store.replace_path_chunks(&rel, updated_chunks);

        // 6. Перезагружаем persistent vector DB, если он был внешне обновлён.
        self.persistent_vector_db = PersistentVectorDb::load_default(&self.root)?;

        // 7. Обновляем indexer
        self.indexer.refresh_file(rel, &self.graph);
        self.indexer.mark_hot(&self.graph, self.vector_store.len());

        Ok(())
    }

    fn refresh_scan_for_saved_file(&mut self, abs: &Path, rel: &Path) {
        self.scan.source_paths.retain(|path| path != abs);
        if detect_language(abs).is_some() {
            self.scan.source_paths.push(abs.to_path_buf());
            self.scan.source_paths.sort();
            self.scan.source_paths.dedup();
        }

        self.scan.config_files.retain(|path| path != rel);
        if is_config_file(abs) {
            self.scan.config_files.push(rel.to_path_buf());
            self.scan.config_files.sort();
            self.scan.config_files.dedup();
        }

        self.scan.entry_candidates.retain(|path| path != rel);
        if is_entry_candidate(abs, rel) {
            self.scan.entry_candidates.push(rel.to_path_buf());
            self.scan.entry_candidates.sort();
            self.scan.entry_candidates.dedup();
        }

        self.scan.language_files.clear();
        for source in &self.scan.source_paths {
            if let Some(language) = detect_language(source) {
                *self.scan.language_files.entry(language).or_insert(0) += 1;
            }
        }
        self.scan.languages = self.scan.language_files.keys().cloned().collect();
        self.scan.source_files = self.scan.source_paths.len();
        self.scan.complexity.source_files = self.scan.source_files;
        self.scan.complexity.language_count = self.scan.languages.len();
    }
}

fn normalize_root(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        Ok(path.canonicalize()?)
    } else {
        anyhow::bail!("project root does not exist: {}", path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn on_file_save_refreshes_changed_file_without_full_project_rechunk() {
        let root = temp_project("on_file_save_refreshes_changed_file");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        let file = src.join("lib.rs");
        fs::write(&file, "pub fn old_name() {}\n").unwrap();

        let mut layer = CognitiveProjectLayer::initialize(&root).unwrap();
        assert!(!layer.symbols.find("old_name").is_empty());

        fs::write(&file, "pub fn new_name() {}\n").unwrap();
        layer.on_file_save(&file).unwrap();

        assert!(layer.symbols.find("old_name").is_empty());
        assert!(!layer.symbols.find("new_name").is_empty());
        assert_eq!(
            layer.vector_store.search("new name", 1)[0].chunk.path,
            PathBuf::from("src/lib.rs")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn initialize_uses_sqlite_snapshot_only_when_fresh() {
        let root = temp_project("initialize_uses_sqlite_snapshot_only_when_fresh");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        let file = src.join("lib.rs");
        fs::write(&file, "pub fn indexed_name() {}\n").unwrap();

        let layer = CognitiveProjectLayer::initialize(&root).unwrap();
        crate::persistent_index::PersistentIndex::build_default(
            &layer.root,
            &layer.scan,
            &layer.symbols,
            &layer.references,
            &layer.graph,
            &layer.vector_store.chunks,
        )
        .unwrap();

        let warm = CognitiveProjectLayer::initialize(&root).unwrap();
        assert!(!warm.symbols.find("indexed_name").is_empty());

        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(&file, "pub fn changed_name() {}\n").unwrap();
        let refreshed = CognitiveProjectLayer::initialize(&root).unwrap();
        assert!(refreshed.symbols.find("indexed_name").is_empty());
        assert!(!refreshed.symbols.find("changed_name").is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn initialize_falls_back_when_sqlite_index_is_unreadable() {
        let root = temp_project("initialize_falls_back_when_sqlite_index_is_unreadable");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        fs::write(src.join("lib.rs"), "pub fn fallback_symbol() {}\n").unwrap();
        fs::create_dir_all(root.join(".cpl")).unwrap();
        fs::write(
            root.join(".cpl").join("index.sqlite"),
            "not a sqlite database",
        )
        .unwrap();

        let layer = CognitiveProjectLayer::initialize(&root).unwrap();
        assert!(!layer.symbols.find("fallback_symbol").is_empty());

        let _ = fs::remove_dir_all(root);
    }

    fn temp_project(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cpl-{name}-{}", unique_suffix()))
    }

    fn unique_suffix() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }
}
