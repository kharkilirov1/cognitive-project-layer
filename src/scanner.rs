use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

pub const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    ".cpl",
    ".ohos",
    ".hvigor",
    "hvigor",
    ".git",
    ".claude",
    ".analyzer",
    "DerivedData",
    "Pods",
    ".gradle",
    ".idea",
    ".vscode",
    ".playwright-mcp",
    "coverage",
    ".env",
    ".env.local",
    ".env.development",
    ".env.production",
    ".env.test",
    "entry/build",
    "oh_modules",
    "__pycache__",
    ".ruff_cache",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ContextMode {
    #[default]
    Full,
    Hybrid,
    Rag,
    Explorer,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectComplexity {
    pub source_files: usize,
    pub estimated_tokens: usize,
    pub language_count: usize,
    pub module_depth: usize,
    pub generated_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectScan {
    pub root: PathBuf,
    pub total_files: usize,
    pub source_files: usize,
    pub total_size_bytes: u64,
    pub languages: Vec<String>,
    pub language_files: BTreeMap<String, usize>,
    pub config_files: Vec<PathBuf>,
    pub entry_candidates: Vec<PathBuf>,
    pub ignored_dirs: Vec<PathBuf>,
    pub git_available: bool,
    pub recent_changed_files: Vec<PathBuf>,
    pub source_paths: Vec<PathBuf>,
    pub complexity: ProjectComplexity,
    pub recommended_mode: ContextMode,
}

impl ProjectScan {
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str("Cognitive Project Layer scan\n");
        out.push_str(&format!("Root: {}\n", self.root.display()));
        out.push_str(&format!(
            "Files: {} total / {} source / {} bytes\n",
            self.total_files, self.source_files, self.total_size_bytes
        ));
        out.push_str(&format!("Languages: {}\n", self.languages.join(", ")));
        out.push_str(&format!("Recommended mode: {:?}\n", self.recommended_mode));
        out.push_str(&format!(
            "Complexity: ~{} tokens, depth {}, generated ratio {:.2}\n",
            self.complexity.estimated_tokens,
            self.complexity.module_depth,
            self.complexity.generated_ratio
        ));
        out.push_str("\nEntry candidates:\n");
        append_paths(&mut out, &self.entry_candidates, 20);
        out.push_str("\nConfig files:\n");
        append_paths(&mut out, &self.config_files, 30);
        out.push_str("\nRecent changed files:\n");
        append_paths(&mut out, &self.recent_changed_files, 30);
        out
    }
}

fn append_paths(out: &mut String, paths: &[PathBuf], limit: usize) {
    if paths.is_empty() {
        out.push_str("- none\n");
        return;
    }
    for path in paths.iter().take(limit) {
        out.push_str(&format!("- {}\n", path.display()));
    }
    if paths.len() > limit {
        out.push_str(&format!("- ... {} more\n", paths.len() - limit));
    }
}

#[derive(Debug, Clone)]
pub struct ProjectScanner {
    ignored_names: BTreeSet<String>,
}

impl Default for ProjectScanner {
    fn default() -> Self {
        Self {
            ignored_names: IGNORED_DIRS.iter().map(|item| item.to_string()).collect(),
        }
    }
}

impl ProjectScanner {
    pub fn scan(&self, root: impl AsRef<Path>) -> Result<ProjectScan> {
        let root = root.as_ref().canonicalize()?;
        let ignore_matcher = IgnoreMatcher::from_root_with_names(&root, self.ignored_names.clone());
        let mut total_files = 0usize;
        let mut source_files = 0usize;
        let mut total_size_bytes = 0u64;
        let mut language_files = BTreeMap::<String, usize>::new();
        let mut config_files = Vec::new();
        let mut entry_candidates = Vec::new();
        let mut ignored_dirs = BTreeSet::<PathBuf>::new();
        let mut source_paths = Vec::new();
        let mut max_depth = 0usize;

        let walker = WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                if ignore_matcher.should_ignore(&root, entry.path()) {
                    if entry.depth() > 0 {
                        ignored_dirs.insert(entry.path().to_path_buf());
                    }
                    false
                } else {
                    true
                }
            });

        for entry in walker {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type().is_dir() {
                max_depth = max_depth.max(entry.depth());
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }

            total_files += 1;
            let metadata = entry.metadata()?;
            total_size_bytes += metadata.len();
            let rel = path.strip_prefix(&root).unwrap_or(path).to_path_buf();

            if is_config_file(path) {
                config_files.push(rel.clone());
            }
            if is_entry_candidate(path, &rel) {
                entry_candidates.push(rel.clone());
            }
            if let Some(language) = detect_language(path) {
                source_files += 1;
                *language_files.entry(language).or_insert(0) += 1;
                source_paths.push(path.to_path_buf());
            }
        }

        config_files.sort();
        entry_candidates.sort();
        source_paths.sort();
        let languages = language_files.keys().cloned().collect::<Vec<_>>();
        let recent_changed_files = recent_changed_files(&root, &ignore_matcher);
        let git_available = is_git_repo(&root);
        let generated_ratio = generated_ratio(total_files, ignored_dirs.len());
        let estimated_tokens = (total_size_bytes / 4) as usize;
        let complexity = ProjectComplexity {
            source_files,
            estimated_tokens,
            language_count: languages.len(),
            module_depth: max_depth,
            generated_ratio,
        };
        let recommended_mode = select_mode(&complexity);

        Ok(ProjectScan {
            root,
            total_files,
            source_files,
            total_size_bytes,
            languages,
            language_files,
            config_files,
            entry_candidates,
            ignored_dirs: ignored_dirs.into_iter().collect(),
            git_available,
            recent_changed_files,
            source_paths,
            complexity,
            recommended_mode,
        })
    }
}

#[derive(Debug, Clone)]
pub struct IgnoreMatcher {
    ignored_names: BTreeSet<String>,
    patterns: Vec<IgnorePattern>,
}

#[derive(Debug, Clone)]
struct IgnorePattern {
    pattern: String,
    anchored: bool,
    directory: bool,
    has_slash: bool,
}

impl IgnoreMatcher {
    pub fn from_root(root: &Path) -> Self {
        Self::from_root_with_names(root, default_ignored_names())
    }

    fn from_root_with_names(root: &Path, ignored_names: BTreeSet<String>) -> Self {
        let mut patterns = Vec::new();
        for file_name in [".gitignore", ".cplignore"] {
            patterns.extend(load_ignore_patterns(&root.join(file_name)));
        }
        Self {
            ignored_names,
            patterns,
        }
    }

    pub fn should_ignore(&self, root: &Path, path: &Path) -> bool {
        if path == root {
            return false;
        }
        if should_ignore_entry_path(path, root, &self.ignored_names) {
            return true;
        }

        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_text = normalize_path_text(rel);
        if rel_text.is_empty() {
            return false;
        }
        self.patterns
            .iter()
            .any(|pattern| pattern.matches(&rel_text))
    }
}

pub fn should_ignore_path(path: &Path) -> bool {
    should_ignore_entry_path(path, Path::new(""), &default_ignored_names())
}

fn should_ignore_entry_path(path: &Path, root: &Path, ignored_names: &BTreeSet<String>) -> bool {
    let match_path = path.strip_prefix(root).unwrap_or(path);
    let normalized_lower = normalize_path_text(match_path);

    ignored_names.iter().any(|ignored| {
        let ignored_norm = ignored.replace('\\', "/").to_ascii_lowercase();
        if ignored_norm.contains('/') {
            normalized_lower == ignored_norm
                || normalized_lower.ends_with(&format!("/{ignored_norm}"))
                || normalized_lower.contains(&format!("/{ignored_norm}/"))
        } else {
            match_path.components().any(|component| {
                component
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&ignored_norm)
            })
        }
    })
}

fn normalize_path_text(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_matches('/')
        .to_ascii_lowercase()
}

fn load_ignore_patterns(path: &Path) -> Vec<IgnorePattern> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(parse_ignore_pattern).collect()
}

fn parse_ignore_pattern(line: &str) -> Option<IgnorePattern> {
    let mut pattern = line.trim();
    if pattern.is_empty() || pattern.starts_with('#') || pattern.starts_with('!') {
        return None;
    }
    if let Some(stripped) = pattern.strip_prefix("./") {
        pattern = stripped;
    }
    let anchored = pattern.starts_with('/');
    if anchored {
        pattern = pattern.trim_start_matches('/');
    }
    let directory = pattern.ends_with('/');
    if directory {
        pattern = pattern.trim_end_matches('/');
    }
    pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }
    let pattern = pattern.replace('\\', "/").to_ascii_lowercase();
    Some(IgnorePattern {
        has_slash: pattern.contains('/'),
        pattern,
        anchored,
        directory,
    })
}

impl IgnorePattern {
    fn matches(&self, rel: &str) -> bool {
        if self.anchored || self.has_slash {
            return path_pattern_matches(&self.pattern, rel, self.directory);
        }

        rel.split('/')
            .any(|component| wildcard_match(&self.pattern, component))
            || path_pattern_matches(&self.pattern, rel, self.directory)
    }
}

fn path_pattern_matches(pattern: &str, rel: &str, directory: bool) -> bool {
    if wildcard_match(pattern, rel) {
        return true;
    }
    if directory {
        rel == pattern
            || rel.starts_with(&format!("{pattern}/"))
            || rel.contains(&format!("/{pattern}/"))
    } else {
        rel == pattern || rel.ends_with(&format!("/{pattern}"))
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }

    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star = None;
    let mut match_after_star = 0usize;

    while t < text.len() {
        if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            match_after_star = t;
            p += 1;
        } else if let Some(star_idx) = star {
            p = star_idx + 1;
            match_after_star += 1;
            t = match_after_star;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn default_ignored_names() -> BTreeSet<String> {
    IGNORED_DIRS.iter().map(|item| item.to_string()).collect()
}

pub fn is_source_file(path: &Path) -> bool {
    detect_language(path).is_some()
}

pub fn is_text_candidate(path: &Path) -> bool {
    is_source_file(path)
        || is_config_file(path)
        || matches!(
            path.extension().and_then(OsStr::to_str),
            Some("md" | "txt" | "toml" | "json" | "json5" | "yaml" | "yml")
        )
}

pub fn detect_language(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let language = match ext.as_str() {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "ets" => "ArkTS",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "C++",
        "c" | "h" => "C/C++",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "dart" => "Dart",
        "cs" => "C#",
        "rb" => "Ruby",
        "php" => "PHP",
        "vue" => "Vue",
        "svelte" => "Svelte",
        _ => return None,
    };
    Some(language.to_string())
}

pub fn is_config_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    matches!(
        name,
        "Cargo.toml"
            | "package.json"
            | "pubspec.yaml"
            | "oh-package.json5"
            | "CMakeLists.txt"
            | "build-profile.json5"
            | "hvigorfile.ts"
            | "module.json5"
            | "tsconfig.json"
            | "vite.config.ts"
            | "next.config.js"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "pom.xml"
            | "build.gradle"
            | "settings.gradle"
    )
}

pub fn is_entry_candidate(path: &Path, rel: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if matches!(
        name,
        "main.rs"
            | "lib.rs"
            | "index.ts"
            | "index.tsx"
            | "main.ts"
            | "main.js"
            | "App.ets"
            | "EntryAbility.ets"
            | "Index.ets"
            | "CMakeLists.txt"
            | "module.json5"
            | "hvigorfile.ts"
    ) {
        return true;
    }
    let rel_text = rel.to_string_lossy().replace('\\', "/");
    matches!(
        rel_text.as_str(),
        "entry/src/main/ets/entryability/EntryAbility.ets"
            | "entry/src/main/ets/pages/Index.ets"
            | "entry/src/main/module.json5"
            | "oh-package.json5"
            | "build-profile.json5"
    )
}

fn is_git_repo(root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn recent_changed_files(root: &Path, ignore_matcher: &IgnoreMatcher) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();

    // git status --porcelain (машиночитаемый формат, не зависит от локали)
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output();
    if let Ok(output) = output
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            // Формат: "XY path" или "XY path -> renamed_path"
            if line.len() > 3 {
                let path_part = &line[3..];
                let path = path_part.trim().trim_matches('"');
                // Для renamed: "path -> newpath" — берём последний
                let path = path.split(" -> ").last().unwrap_or(path);
                if !path.is_empty() && !ignore_matcher.should_ignore(root, Path::new(path)) {
                    files.insert(path.to_string());
                }
            }
        }
    }

    // git diff --name-only (без HEAD~5, чтобы не падать на новых репозиториях)
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--count", "HEAD"])
        .output();
    let commit_count = output
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(0);

    if commit_count >= 5 {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["diff", "--name-only", "HEAD~5..HEAD"])
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let line = line.trim();
                if !line.is_empty() && !ignore_matcher.should_ignore(root, Path::new(line)) {
                    files.insert(line.to_string());
                }
            }
        }
    }

    files.into_iter().map(PathBuf::from).collect()
}

fn generated_ratio(total_files: usize, ignored_dirs: usize) -> f32 {
    if total_files == 0 {
        return 0.0;
    }
    (ignored_dirs as f32 / (total_files + ignored_dirs) as f32).clamp(0.0, 1.0)
}

fn select_mode(complexity: &ProjectComplexity) -> ContextMode {
    if complexity.source_files == 0 {
        ContextMode::Explorer
    } else if complexity.source_files <= 80 && complexity.estimated_tokens <= 120_000 {
        ContextMode::Full
    } else if complexity.source_files <= 1_000 && complexity.estimated_tokens <= 1_500_000 {
        ContextMode::Hybrid
    } else {
        ContextMode::Rag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_harmonyos_entry_candidates() {
        assert!(is_entry_candidate(
            Path::new("EntryAbility.ets"),
            Path::new("entry/src/main/ets/entryability/EntryAbility.ets")
        ));
        assert!(is_config_file(Path::new("oh-package.json5")));
        assert_eq!(
            detect_language(Path::new("Index.ets")).as_deref(),
            Some("ArkTS")
        );
    }

    #[test]
    fn ignores_cpl_cache_and_nested_build_paths() {
        assert!(should_ignore_path(Path::new(".cpl/vector_db.json")));
        assert!(should_ignore_path(Path::new(".playwright-mcp/page.yml")));
        assert!(should_ignore_path(Path::new("entry/build/output.js")));
        assert!(!should_ignore_path(Path::new("src/build_tool.rs")));
    }

    #[test]
    fn scan_excludes_local_cpl_cache_from_size_and_mode() {
        let root = temp_project("scan_excludes_local_cpl_cache");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".cpl")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn real_api() {}\n").unwrap();
        std::fs::write(root.join(".cpl/vector_db.json"), "x".repeat(2_000_000)).unwrap();

        let scan = ProjectScanner::default().scan(&root).unwrap();

        assert_eq!(scan.source_files, 1);
        assert_eq!(scan.recommended_mode, ContextMode::Full);
        assert!(
            scan.ignored_dirs
                .iter()
                .any(|path| path.file_name().and_then(OsStr::to_str) == Some(".cpl"))
        );
        assert!(scan.total_size_bytes < 100_000);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_respects_cplignore_and_gitignore_patterns() {
        let root = temp_project("scan_respects_ignore_files");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::create_dir_all(root.join("ignored-by-git")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn real_api() {}\n").unwrap();
        std::fs::write(
            root.join("generated/noisy.rs"),
            "pub fn generated_api() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("ignored-by-git/hidden.rs"),
            "pub fn ignored_api() {}\n",
        )
        .unwrap();
        std::fs::write(root.join(".cplignore"), "generated/\n*.snap\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored-by-git/\n").unwrap();

        let scan = ProjectScanner::default().scan(&root).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        let rel_sources = scan
            .source_paths
            .iter()
            .map(|path| {
                path.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();

        assert_eq!(rel_sources, vec!["src/lib.rs"]);
        assert!(
            scan.ignored_dirs
                .iter()
                .any(|path| path.ends_with("generated"))
        );
        assert!(
            scan.ignored_dirs
                .iter()
                .any(|path| path.ends_with("ignored-by-git"))
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignored_parent_directory_names_do_not_hide_project_root() {
        let root = temp_project("ignored_parent_directory_names")
            .join("build")
            .join("actual-project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn real_api() {}\n").unwrap();

        let scan = ProjectScanner::default().scan(&root).unwrap();

        assert_eq!(scan.source_files, 1);
        assert_eq!(scan.languages, vec!["Rust"]);

        let cleanup = root
            .ancestors()
            .nth(2)
            .expect("temp root ancestor")
            .to_path_buf();
        std::fs::remove_dir_all(cleanup).unwrap();
    }

    #[test]
    fn wildcard_ignore_patterns_match_file_names() {
        let pattern = parse_ignore_pattern("*.snap").unwrap();
        assert!(pattern.matches("tests/output.snap"));
        assert!(!pattern.matches("tests/output.rs"));
    }

    fn temp_project(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cpl-{name}-{}", unique_suffix()))
    }

    fn unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }
}
