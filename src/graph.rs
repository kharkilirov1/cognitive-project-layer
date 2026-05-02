use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::chunk::extract_imports;
use crate::scanner::{ProjectScan, is_config_file, is_source_file};
use crate::symbols::{SymbolIndex, SymbolKind};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum GraphNodeKind {
    File,
    Symbol,
    Module,
    Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: GraphNodeKind,
    pub label: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GraphEdgeKind {
    Imports,
    Exports,
    Calls,
    UsesComponent,
    Tests,
    Configures,
    NativeBinding,
    Contains,
    InModule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: GraphEdgeKind,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub file_nodes: BTreeMap<PathBuf, String>,
}

impl ProjectGraph {
    pub fn build(root: &Path, scan: &ProjectScan, symbols: &SymbolIndex) -> Result<Self> {
        let mut graph = Self::default();
        let mut node_ids = BTreeSet::new();

        for abs in &scan.source_paths {
            let rel = abs.strip_prefix(root).unwrap_or(abs).to_path_buf();
            graph.add_node(
                &mut node_ids,
                GraphNode {
                    id: file_id(&rel),
                    kind: GraphNodeKind::File,
                    label: rel.display().to_string(),
                    path: Some(rel.clone()),
                },
            );
            graph.file_nodes.insert(rel.clone(), file_id(&rel));

            if let Some(module) = rel.parent() {
                let module_path = module.to_path_buf();
                let module_id = module_id(&module_path);
                graph.add_node(
                    &mut node_ids,
                    GraphNode {
                        id: module_id.clone(),
                        kind: GraphNodeKind::Module,
                        label: module.display().to_string(),
                        path: Some(module_path),
                    },
                );
                graph.add_edge(
                    module_id,
                    file_id(&rel),
                    GraphEdgeKind::InModule,
                    "file belongs to module",
                );
            }
        }

        for rel in &scan.config_files {
            graph.add_node(
                &mut node_ids,
                GraphNode {
                    id: config_id(rel),
                    kind: GraphNodeKind::Config,
                    label: rel.display().to_string(),
                    path: Some(rel.clone()),
                },
            );
            for module in top_level_modules(&graph.file_nodes) {
                graph.add_edge(
                    config_id(rel),
                    module_id(&module),
                    GraphEdgeKind::Configures,
                    "config affects module",
                );
            }
        }

        for symbol in &symbols.symbols {
            let rel = symbol
                .path
                .strip_prefix(root)
                .unwrap_or(&symbol.path)
                .to_path_buf();
            let sid = symbol_id(&rel, &symbol.name, symbol.line_start);
            graph.add_node(
                &mut node_ids,
                GraphNode {
                    id: sid.clone(),
                    kind: GraphNodeKind::Symbol,
                    label: symbol.name.clone(),
                    path: Some(rel.clone()),
                },
            );
            graph.add_edge(
                file_id(&rel),
                sid,
                GraphEdgeKind::Contains,
                &symbol.signature,
            );
        }

        for abs in &scan.source_paths {
            let rel = abs.strip_prefix(root).unwrap_or(abs).to_path_buf();
            let Ok(source) = fs::read_to_string(abs) else {
                continue;
            };
            for import in extract_imports(&source) {
                if let Some(target) = resolve_import(root, &rel, &import, scan) {
                    graph.add_edge(
                        file_id(&rel),
                        file_id(&target),
                        GraphEdgeKind::Imports,
                        &import,
                    );
                }
            }
            for test_target in infer_test_target(&rel, scan) {
                graph.add_edge(
                    file_id(&rel),
                    file_id(&test_target),
                    GraphEdgeKind::Tests,
                    "test/source naming convention",
                );
            }
            graph.add_callish_edges(root, &rel, &source, symbols);
        }

        graph.dedup_edges();
        Ok(graph)
    }

    pub fn refresh_file(
        &mut self,
        root: &Path,
        scan: &ProjectScan,
        symbols: &SymbolIndex,
        abs: &Path,
    ) -> Result<()> {
        let rel = abs.strip_prefix(root).unwrap_or(abs).to_path_buf();
        self.remove_file_surface(&rel);
        if !abs.exists() {
            self.dedup_edges();
            return Ok(());
        }

        let mut node_ids = self
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();
        self.add_file_surface(root, scan, symbols, &rel, abs, &mut node_ids)?;
        self.dedup_edges();
        Ok(())
    }

    pub fn expand_files(&self, seeds: &[PathBuf], depth: usize) -> Vec<PathBuf> {
        let mut visited = BTreeSet::<String>::new();
        let mut queue = VecDeque::<(String, usize)>::new();
        let mut files = BTreeSet::<PathBuf>::new();

        for seed in seeds {
            let id = file_id(seed);
            if self.nodes.iter().any(|node| node.id == id) {
                queue.push_back((id.clone(), 0));
                visited.insert(id);
            }
        }

        while let Some((id, current_depth)) = queue.pop_front() {
            if let Some(path) = self.path_for_id(&id) {
                files.insert(path);
            }
            if current_depth >= depth {
                continue;
            }
            for neighbor in self.neighbors(&id) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back((neighbor, current_depth + 1));
                }
            }
        }

        files.into_iter().collect()
    }

    pub fn related_to_symbol_paths(&self, symbols: &[String], depth: usize) -> Vec<PathBuf> {
        let seed_files = self
            .nodes
            .iter()
            .filter(|node| node.kind == GraphNodeKind::Symbol && symbols.contains(&node.label))
            .filter_map(|node| node.path.clone())
            .collect::<Vec<_>>();
        self.expand_files(&seed_files, depth)
    }

    pub fn render_summary(&self) -> String {
        let mut by_kind = BTreeMap::<String, usize>::new();
        for node in &self.nodes {
            *by_kind.entry(format!("{:?}", node.kind)).or_insert(0) += 1;
        }
        let mut edge_kind = BTreeMap::<String, usize>::new();
        for edge in &self.edges {
            *edge_kind.entry(format!("{:?}", edge.kind)).or_insert(0) += 1;
        }

        let mut out = String::new();
        out.push_str("Project graph\n");
        out.push_str(&format!(
            "Nodes: {}; edges: {}\n",
            self.nodes.len(),
            self.edges.len()
        ));
        out.push_str(&format!("Node kinds: {}\n", join_counts(&by_kind)));
        out.push_str(&format!("Edge kinds: {}\n", join_counts(&edge_kind)));
        out
    }

    fn add_node(&mut self, node_ids: &mut BTreeSet<String>, node: GraphNode) {
        if node_ids.insert(node.id.clone()) {
            self.nodes.push(node);
        }
    }

    fn add_edge(
        &mut self,
        from: String,
        to: String,
        kind: GraphEdgeKind,
        evidence: impl Into<String>,
    ) {
        if from != to {
            self.edges.push(GraphEdge {
                from,
                to,
                kind,
                evidence: evidence.into(),
            });
        }
    }

    fn dedup_edges(&mut self) {
        let mut seen = BTreeSet::new();
        self.edges.retain(|edge| {
            seen.insert((
                edge.from.clone(),
                edge.to.clone(),
                format!("{:?}", edge.kind),
                edge.evidence.clone(),
            ))
        });
    }

    fn neighbors(&self, id: &str) -> Vec<String> {
        let mut neighbors = Vec::new();
        for edge in &self.edges {
            if edge.from == id {
                neighbors.push(edge.to.clone());
            }
            if edge.to == id {
                neighbors.push(edge.from.clone());
            }
        }
        neighbors
    }

    fn path_for_id(&self, id: &str) -> Option<PathBuf> {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .and_then(|node| node.path.clone())
            .filter(|_| id.starts_with("file:"))
    }

    fn add_file_surface(
        &mut self,
        root: &Path,
        scan: &ProjectScan,
        symbols: &SymbolIndex,
        rel: &Path,
        abs: &Path,
        node_ids: &mut BTreeSet<String>,
    ) -> Result<()> {
        if is_config_file(abs) {
            self.add_node(
                node_ids,
                GraphNode {
                    id: config_id(rel),
                    kind: GraphNodeKind::Config,
                    label: rel.display().to_string(),
                    path: Some(rel.to_path_buf()),
                },
            );
            for module in top_level_modules(&self.file_nodes) {
                self.add_edge(
                    config_id(rel),
                    module_id(&module),
                    GraphEdgeKind::Configures,
                    "config affects module",
                );
            }
            return Ok(());
        }

        if !is_source_file(abs) {
            return Ok(());
        }

        self.add_node(
            node_ids,
            GraphNode {
                id: file_id(rel),
                kind: GraphNodeKind::File,
                label: rel.display().to_string(),
                path: Some(rel.to_path_buf()),
            },
        );
        self.file_nodes.insert(rel.to_path_buf(), file_id(rel));

        if let Some(module) = rel.parent() {
            let module_path = module.to_path_buf();
            let module_id = module_id(&module_path);
            self.add_node(
                node_ids,
                GraphNode {
                    id: module_id.clone(),
                    kind: GraphNodeKind::Module,
                    label: module.display().to_string(),
                    path: Some(module_path),
                },
            );
            self.add_edge(
                module_id,
                file_id(rel),
                GraphEdgeKind::InModule,
                "file belongs to module",
            );
        }

        for symbol in &symbols.symbols {
            let symbol_rel = symbol
                .path
                .strip_prefix(root)
                .unwrap_or(&symbol.path)
                .to_path_buf();
            if symbol_rel != rel {
                continue;
            }
            let sid = symbol_id(rel, &symbol.name, symbol.line_start);
            self.add_node(
                node_ids,
                GraphNode {
                    id: sid.clone(),
                    kind: GraphNodeKind::Symbol,
                    label: symbol.name.clone(),
                    path: Some(rel.to_path_buf()),
                },
            );
            self.add_edge(
                file_id(rel),
                sid,
                GraphEdgeKind::Contains,
                &symbol.signature,
            );
        }

        let Ok(source) = fs::read_to_string(abs) else {
            return Ok(());
        };
        for import in extract_imports(&source) {
            if let Some(target) = resolve_import(root, rel, &import, scan) {
                self.add_edge(
                    file_id(rel),
                    file_id(&target),
                    GraphEdgeKind::Imports,
                    &import,
                );
            }
        }
        for test_target in infer_test_target(rel, scan) {
            self.add_edge(
                file_id(rel),
                file_id(&test_target),
                GraphEdgeKind::Tests,
                "test/source naming convention",
            );
        }
        self.add_callish_edges(root, rel, &source, symbols);
        Ok(())
    }

    fn remove_file_surface(&mut self, rel: &Path) {
        let mut removed_ids = BTreeSet::from([file_id(rel), config_id(rel)]);
        self.nodes.retain(|node| {
            let remove = node.path.as_deref() == Some(rel)
                && matches!(
                    node.kind,
                    GraphNodeKind::File | GraphNodeKind::Symbol | GraphNodeKind::Config
                );
            if remove {
                removed_ids.insert(node.id.clone());
            }
            !remove
        });
        self.file_nodes.remove(rel);
        self.edges
            .retain(|edge| !removed_ids.contains(&edge.from) && !removed_ids.contains(&edge.to));
    }

    fn add_callish_edges(
        &mut self,
        root: &Path,
        from_rel: &Path,
        source: &str,
        symbols: &SymbolIndex,
    ) {
        let lower = source.to_lowercase();
        let (reference_names, component_tag_names) = callish_reference_names(source);

        for name in reference_names {
            let Some(indices) = symbols.by_name.get(&name) else {
                continue;
            };
            for idx in indices {
                let Some(symbol) = symbols.symbols.get(*idx) else {
                    continue;
                };
                let target_rel = symbol
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&symbol.path)
                    .to_path_buf();
                if target_rel == from_rel {
                    continue;
                }

                let kind = if matches!(symbol.kind, SymbolKind::Component)
                    || component_tag_names.contains(&name)
                {
                    GraphEdgeKind::UsesComponent
                } else {
                    GraphEdgeKind::Calls
                };
                self.add_edge(
                    file_id(from_rel),
                    symbol_id(&target_rel, &symbol.name, symbol.line_start),
                    kind,
                    "call-ish textual reference",
                );
            }
        }

        if lower.contains("napi") || lower.contains("extern \"c\"") || lower.contains("hilog") {
            for config in self
                .nodes
                .iter()
                .filter(|node| node.kind == GraphNodeKind::Config)
                .map(|node| node.id.clone())
                .collect::<Vec<_>>()
            {
                self.add_edge(
                    file_id(from_rel),
                    config,
                    GraphEdgeKind::NativeBinding,
                    "native/NAPI/Hilog textual marker",
                );
            }
        }
    }
}

fn callish_reference_names(source: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut names = BTreeSet::new();
    let mut component_tags = BTreeSet::new();
    let mut idx = 0usize;

    while idx < chars.len() {
        if chars[idx] == '<' {
            let start = idx + 1;
            if start < chars.len() && is_ident_start(chars[start]) {
                let (name, end) = read_identifier(&chars, start);
                if !name.is_empty() {
                    component_tags.insert(name.clone());
                    names.insert(name);
                }
                idx = end;
                continue;
            }
        }

        if is_ident_start(chars[idx]) {
            let (name, end) = read_identifier(&chars, idx);
            let next = skip_whitespace(&chars, end);
            if next < chars.len() && matches!(chars[next], '(' | '{') {
                names.insert(name);
            }
            idx = end;
            continue;
        }

        idx += 1;
    }

    (names, component_tags)
}

fn read_identifier(chars: &[char], start: usize) -> (String, usize) {
    let mut end = start;
    while end < chars.len() && is_ident_continue(chars[end]) {
        end += 1;
    }
    (chars[start..end].iter().collect::<String>(), end)
}

fn skip_whitespace(chars: &[char], mut idx: usize) -> usize {
    while idx < chars.len() && chars[idx].is_whitespace() {
        idx += 1;
    }
    idx
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

fn resolve_import(
    root: &Path,
    from_rel: &Path,
    import: &str,
    scan: &ProjectScan,
) -> Option<PathBuf> {
    let candidates = import_candidates(root, from_rel, import);
    candidates.into_iter().find(|candidate| {
        scan.source_paths
            .iter()
            .any(|source| source.strip_prefix(root).unwrap_or(source) == candidate)
    })
}

fn import_candidates(_root: &Path, from_rel: &Path, import: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let parent = from_rel.parent().unwrap_or_else(|| Path::new(""));

    if import.starts_with('.') {
        let base = normalize_rel(parent.join(import));
        push_with_extensions(&mut candidates, &base);
        push_index_candidates(&mut candidates, &base);
    } else if !import.contains("::") && !import.contains('/') && !import.contains('.') {
        let base = parent.join(import);
        push_with_extensions(&mut candidates, &base);
        candidates.push(PathBuf::from("src").join(format!("{import}.rs")));
        candidates.push(PathBuf::from("src").join(import).join("mod.rs"));
    } else if let Some(crate_path) = import.strip_prefix("crate::") {
        let parts = crate_path.split("::").collect::<Vec<_>>();
        if let Some(first) = parts.first() {
            candidates.push(PathBuf::from("src").join(format!("{first}.rs")));
            candidates.push(PathBuf::from("src").join(first).join("mod.rs"));
        }
    }

    candidates.into_iter().map(normalize_rel).collect()
}

fn push_with_extensions(candidates: &mut Vec<PathBuf>, base: &Path) {
    candidates.push(base.to_path_buf());
    for ext in [
        "ts", "tsx", "js", "jsx", "ets", "rs", "py", "go", "cpp", "h", "hpp",
    ] {
        candidates.push(base.with_extension(ext));
    }
}

fn push_index_candidates(candidates: &mut Vec<PathBuf>, base: &Path) {
    for name in [
        "index.ts",
        "index.tsx",
        "index.js",
        "Index.ets",
        "mod.rs",
        "__init__.py",
    ] {
        candidates.push(base.join(name));
    }
}

fn normalize_rel(path: PathBuf) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

fn infer_test_target(test_path: &Path, scan: &ProjectScan) -> Vec<PathBuf> {
    let text = test_path.to_string_lossy().to_lowercase();
    if !text.contains("test") && !text.contains("spec") {
        return Vec::new();
    }
    let stem = test_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .replace("_test", "")
        .replace(".test", "")
        .replace(".spec", "");
    scan.source_paths
        .iter()
        .filter_map(|source| {
            source
                .file_stem()
                .and_then(|source_stem| source_stem.to_str())
                .map(|s| (source, s))
        })
        .filter(|(_, source_stem)| *source_stem == stem)
        .map(|(source, _)| {
            source
                .strip_prefix(&scan.root)
                .unwrap_or(source)
                .to_path_buf()
        })
        .filter(|source| source != test_path)
        .collect()
}

fn top_level_modules(file_nodes: &BTreeMap<PathBuf, String>) -> Vec<PathBuf> {
    let mut modules = BTreeSet::new();
    for path in file_nodes.keys() {
        if let Some(first) = path.components().next() {
            modules.insert(PathBuf::from(first.as_os_str()));
        }
    }
    modules.into_iter().collect()
}

fn file_id(path: &Path) -> String {
    format!("file:{}", path.to_string_lossy().replace('\\', "/"))
}

fn config_id(path: &Path) -> String {
    format!("config:{}", path.to_string_lossy().replace('\\', "/"))
}

fn module_id(path: &Path) -> String {
    format!("module:{}", path.to_string_lossy().replace('\\', "/"))
}

fn symbol_id(path: &Path, name: &str, line: usize) -> String {
    format!(
        "symbol:{}:{name}:{line}",
        path.to_string_lossy().replace('\\', "/")
    )
}

fn join_counts(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        "none".to_string()
    } else {
        counts
            .iter()
            .map(|(kind, count)| format!("{kind}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_import_candidates() {
        let candidates = import_candidates(
            Path::new("."),
            Path::new("src/ui/Page.ts"),
            "./components/Button",
        );
        assert!(candidates.contains(&PathBuf::from("src/ui/components/Button.ts")));
        assert!(candidates.contains(&PathBuf::from("src/ui/components/Button/index.ts")));
    }

    #[test]
    fn extracts_only_callish_symbol_references() {
        let (names, component_tags) =
            callish_reference_names("let x = token; login(user); Foo { id }; <TgRow />");
        assert!(names.contains("login"));
        assert!(names.contains("Foo"));
        assert!(names.contains("TgRow"));
        assert!(component_tags.contains("TgRow"));
        assert!(!names.contains("token"));
    }
}
