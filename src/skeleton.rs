use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::scanner::{ContextMode, ProjectScan};
use crate::symbols::{SymbolIndex, SymbolKind, SymbolLocation, Visibility};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectMeta {
    pub name: String,
    pub root: PathBuf,
    pub language_stack: Vec<String>,
    pub total_files: usize,
    pub source_files: usize,
    pub mode: ContextMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryPoint {
    pub path: PathBuf,
    pub kind: EntryKind,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntryKind {
    Application,
    Library,
    HarmonyAbility,
    HarmonyPage,
    Config,
    Build,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleNode {
    pub path: PathBuf,
    pub name: String,
    pub purpose: Option<String>,
    pub key_files: Vec<PathBuf>,
    pub source_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSurface {
    pub symbol_name: String,
    pub kind: SymbolKind,
    pub path: PathBuf,
    pub line_start: usize,
    pub signature: String,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub kind: ConfigKind,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigKind {
    RustCargo,
    NodePackage,
    HarmonyPackage,
    HarmonyModule,
    HarmonyBuildProfile,
    Hvigor,
    CMake,
    Python,
    Go,
    Gradle,
    TypeScript,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffSummary {
    pub path: PathBuf,
    pub change_type: ChangeType,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeType {
    Modified,
    Added,
    Deleted,
    Renamed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SymbolSummary {
    pub total: usize,
    pub public_api: usize,
    pub by_kind: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Skeleton {
    pub project: ProjectMeta,
    pub entry_points: Vec<EntryPoint>,
    pub modules: Vec<ModuleNode>,
    pub public_api: Vec<ApiSurface>,
    pub configs: Vec<ConfigFile>,
    pub recent_changes: Vec<GitDiffSummary>,
    pub symbol_summary: SymbolSummary,
    pub important_paths: Vec<PathBuf>,
}

impl Skeleton {
    pub fn build(scan: &ProjectScan, symbols: &SymbolIndex) -> Self {
        let project_name = scan
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
            .to_string();
        let project = ProjectMeta {
            name: project_name,
            root: scan.root.clone(),
            language_stack: scan.languages.clone(),
            total_files: scan.total_files,
            source_files: scan.source_files,
            mode: scan.recommended_mode.clone(),
        };
        let entry_points = scan
            .entry_candidates
            .iter()
            .map(|path| EntryPoint {
                path: path.clone(),
                kind: classify_entry(path),
                summary: summarize_entry(path),
            })
            .collect::<Vec<_>>();
        let modules = build_modules(scan);
        let public_api = build_public_api(symbols);
        let configs = scan
            .config_files
            .iter()
            .map(|path| ConfigFile {
                path: path.clone(),
                kind: classify_config(path),
                summary: summarize_config(path),
            })
            .collect::<Vec<_>>();
        let recent_changes = scan
            .recent_changed_files
            .iter()
            .map(|path| GitDiffSummary {
                path: path.clone(),
                change_type: ChangeType::Modified,
                summary: format!("recent git change: {}", path.display()),
            })
            .collect::<Vec<_>>();
        let symbol_summary = SymbolSummary {
            total: symbols.symbols.len(),
            public_api: symbols.public_symbols().len(),
            by_kind: symbols.summary_counts(),
        };
        let mut important_paths = BTreeSet::new();
        for entry in &entry_points {
            important_paths.insert(entry.path.clone());
        }
        for config in &configs {
            important_paths.insert(config.path.clone());
        }
        for change in &recent_changes {
            important_paths.insert(change.path.clone());
        }

        Self {
            project,
            entry_points,
            modules,
            public_api,
            configs,
            recent_changes,
            symbol_summary,
            important_paths: important_paths.into_iter().collect(),
        }
    }

    pub fn render_prompt(&self) -> String {
        let mut out = String::new();
        out.push_str("# Cognitive Project Skeleton\n");
        out.push_str(&format!(
            "Project: {} ({})\n",
            self.project.name,
            self.project.root.display()
        ));
        out.push_str(&format!(
            "Mode: {:?}; files: {} total / {} source; languages: {}\n",
            self.project.mode,
            self.project.total_files,
            self.project.source_files,
            list_or_none(&self.project.language_stack)
        ));
        out.push_str(&format!(
            "Symbols: {} total / {} public; kinds: {}\n",
            self.symbol_summary.total,
            self.symbol_summary.public_api,
            self.symbol_summary
                .by_kind
                .iter()
                .map(|(kind, count)| format!("{kind}={count}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));

        out.push_str("\nEntry points:\n");
        render_limited(&mut out, &self.entry_points, 12, |entry| {
            format!(
                "- {} [{:?}] — {}",
                entry.path.display(),
                entry.kind,
                entry.summary
            )
        });

        out.push_str("\nModules:\n");
        render_limited(&mut out, &self.modules, 16, |module| {
            let key_files = module
                .key_files
                .iter()
                .take(3)
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "- {} — {} source files{}",
                module.path.display(),
                module.source_files,
                if key_files.is_empty() {
                    String::new()
                } else {
                    format!("; key: {key_files}")
                }
            )
        });

        out.push_str("\nPublic API candidates:\n");
        render_limited(&mut out, &self.public_api, 20, |api| {
            format!(
                "- {} {:?} at {}:{} — {}",
                api.symbol_name,
                api.kind,
                api.path.display(),
                api.line_start,
                trim(&api.signature, 100)
            )
        });

        out.push_str("\nConfigs:\n");
        render_limited(&mut out, &self.configs, 16, |config| {
            format!(
                "- {} [{:?}] — {}",
                config.path.display(),
                config.kind,
                config.summary
            )
        });

        out.push_str("\nRecent changes:\n");
        render_limited(&mut out, &self.recent_changes, 12, |change| {
            format!(
                "- {} [{:?}] — {}",
                change.path.display(),
                change.change_type,
                change.summary
            )
        });

        out
    }
}

fn build_modules(scan: &ProjectScan) -> Vec<ModuleNode> {
    let mut buckets = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for abs_path in &scan.source_paths {
        let rel = abs_path.strip_prefix(&scan.root).unwrap_or(abs_path);
        let module_path = module_bucket(rel);
        buckets
            .entry(module_path)
            .or_default()
            .push(rel.to_path_buf());
    }

    buckets
        .into_iter()
        .map(|(path, mut files)| {
            files.sort();
            ModuleNode {
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(".")
                    .to_string(),
                purpose: infer_module_purpose(&path),
                path,
                source_files: files.len(),
                key_files: files.into_iter().take(5).collect(),
            }
        })
        .collect()
}

fn module_bucket(rel: &Path) -> PathBuf {
    let mut components = rel.components();
    let first = components.next().map(|c| c.as_os_str().to_os_string());
    let second = components.next().map(|c| c.as_os_str().to_os_string());
    match (first, second) {
        (Some(first), Some(second)) if rel.components().count() > 2 => {
            PathBuf::from(first).join(second)
        }
        (Some(first), _) => PathBuf::from(first),
        _ => PathBuf::from("."),
    }
}

fn build_public_api(symbols: &SymbolIndex) -> Vec<ApiSurface> {
    let mut public = symbols
        .public_symbols()
        .into_iter()
        .chain(
            symbols
                .symbols
                .iter()
                .filter(|symbol| is_exportish(symbol))
                .cloned(),
        )
        .map(|symbol| ApiSurface {
            symbol_name: symbol.name,
            kind: symbol.kind,
            path: symbol.path,
            line_start: symbol.line_start,
            signature: symbol.signature,
            visibility: symbol.visibility,
        })
        .collect::<Vec<_>>();
    public.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
    });
    public.dedup_by(|left, right| {
        left.symbol_name == right.symbol_name && left.path == right.path && left.kind == right.kind
    });
    public.truncate(80);
    public
}

fn is_exportish(symbol: &SymbolLocation) -> bool {
    matches!(
        symbol.kind,
        SymbolKind::Component | SymbolKind::Class | SymbolKind::Interface | SymbolKind::Export
    ) && symbol.visibility != Visibility::Internal
}

fn classify_entry(path: &Path) -> EntryKind {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.ends_with("EntryAbility.ets") {
        EntryKind::HarmonyAbility
    } else if text.ends_with("Index.ets") || text.ends_with("App.ets") {
        EntryKind::HarmonyPage
    } else if text.ends_with("module.json5") || text.ends_with("oh-package.json5") {
        EntryKind::Config
    } else if text.ends_with("CMakeLists.txt") || text.ends_with("hvigorfile.ts") {
        EntryKind::Build
    } else if text.ends_with("lib.rs") {
        EntryKind::Library
    } else if text.ends_with("main.rs") || text.ends_with("main.ts") || text.ends_with("index.ts") {
        EntryKind::Application
    } else {
        EntryKind::Unknown
    }
}

fn summarize_entry(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.contains("entryability") {
        "HarmonyOS UIAbility entry lifecycle".to_string()
    } else if text.contains("/pages/") || text.ends_with("Index.ets") {
        "HarmonyOS page/component entry".to_string()
    } else if text.ends_with("main.rs") {
        "Rust binary entry point".to_string()
    } else if text.ends_with("lib.rs") {
        "Rust library public surface".to_string()
    } else if text.ends_with("CMakeLists.txt") {
        "Native build surface".to_string()
    } else {
        "project entry/config candidate".to_string()
    }
}

fn classify_config(path: &Path) -> ConfigKind {
    match path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
    {
        "Cargo.toml" => ConfigKind::RustCargo,
        "package.json" => ConfigKind::NodePackage,
        "oh-package.json5" => ConfigKind::HarmonyPackage,
        "module.json5" => ConfigKind::HarmonyModule,
        "build-profile.json5" => ConfigKind::HarmonyBuildProfile,
        "hvigorfile.ts" => ConfigKind::Hvigor,
        "CMakeLists.txt" => ConfigKind::CMake,
        "pyproject.toml" | "requirements.txt" => ConfigKind::Python,
        "go.mod" => ConfigKind::Go,
        "build.gradle" | "settings.gradle" => ConfigKind::Gradle,
        "tsconfig.json" => ConfigKind::TypeScript,
        _ => ConfigKind::Unknown,
    }
}

fn summarize_config(path: &Path) -> String {
    match classify_config(path) {
        ConfigKind::RustCargo => "Rust package/dependencies".to_string(),
        ConfigKind::NodePackage => "Node package scripts/dependencies".to_string(),
        ConfigKind::HarmonyPackage => "HarmonyOS package dependencies".to_string(),
        ConfigKind::HarmonyModule => "HarmonyOS module abilities/pages/config".to_string(),
        ConfigKind::HarmonyBuildProfile => "HarmonyOS build profile".to_string(),
        ConfigKind::Hvigor => "HarmonyOS Hvigor build entry".to_string(),
        ConfigKind::CMake => "C/C++ native build config".to_string(),
        ConfigKind::Python => "Python package/dependencies".to_string(),
        ConfigKind::Go => "Go module dependencies".to_string(),
        ConfigKind::Gradle => "Gradle build config".to_string(),
        ConfigKind::TypeScript => "TypeScript compiler config".to_string(),
        ConfigKind::Unknown => "configuration surface".to_string(),
    }
}

fn infer_module_purpose(path: &Path) -> Option<String> {
    let text = path.to_string_lossy().to_ascii_lowercase();
    let purpose = if text.contains("auth") {
        "authentication / authorization"
    } else if text.contains("db") || text.contains("repo") {
        "database / persistence"
    } else if text.contains("api") || text.contains("route") {
        "HTTP/API routing"
    } else if text.contains("ui") || text.contains("page") || text.contains("component") {
        "UI components / pages"
    } else if text.contains("native") || text.contains("napi") || text.contains("cpp") {
        "native bridge / C++"
    } else if text.contains("test") || text.contains("spec") {
        "tests"
    } else {
        return None;
    };
    Some(purpose.to_string())
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn render_limited<T, F>(out: &mut String, items: &[T], limit: usize, render: F)
where
    F: Fn(&T) -> String,
{
    if items.is_empty() {
        out.push_str("- none\n");
        return;
    }
    for item in items.iter().take(limit) {
        out.push_str(&render(item));
        out.push('\n');
    }
    if items.len() > limit {
        out.push_str(&format!("- ... {} more\n", items.len() - limit));
    }
}

fn trim(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        format!("{}…", &text[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_harmony_configs() {
        assert!(matches!(
            classify_config(Path::new("entry/src/main/module.json5")),
            ConfigKind::HarmonyModule
        ));
        assert!(matches!(
            classify_entry(Path::new(
                "entry/src/main/ets/entryability/EntryAbility.ets"
            )),
            EntryKind::HarmonyAbility
        ));
    }
}
