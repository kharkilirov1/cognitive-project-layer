use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::Result;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::scanner::{is_text_candidate, should_ignore_path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: PathBuf,
    pub line_number: usize,
    pub line: String,
}

pub struct FallbackTools;

impl FallbackTools {
    pub fn file_tree(root: impl AsRef<Path>, max_depth: usize) -> Result<String> {
        let root = root.as_ref().canonicalize()?;
        let mut lines = vec![format!("{}/", root.display())];
        let mut seen_dirs = BTreeSet::new();

        for entry in WalkDir::new(&root)
            .follow_links(false)
            .max_depth(max_depth + 1)
            .into_iter()
            .filter_entry(|entry| !should_ignore_path(entry.path()))
        {
            let entry = entry?;
            if entry.depth() == 0 {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&root).unwrap_or(path);
            if should_ignore_path(rel) {
                continue;
            }
            if entry.file_type().is_dir() && !seen_dirs.insert(rel.to_path_buf()) {
                continue;
            }
            let indent = "  ".repeat(entry.depth().saturating_sub(1));
            let suffix = if entry.file_type().is_dir() { "/" } else { "" };
            lines.push(format!(
                "{indent}{}{}",
                entry.file_name().to_string_lossy(),
                suffix
            ));
        }

        Ok(lines.join("\n"))
    }

    pub fn grep(root: impl AsRef<Path>, pattern: &str, limit: usize) -> Result<Vec<GrepMatch>> {
        let root = root.as_ref().canonicalize()?;
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .or_else(|_| {
                RegexBuilder::new(&regex::escape(pattern))
                    .case_insensitive(true)
                    .build()
            })?;
        let mut matches = Vec::new();

        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !should_ignore_path(entry.path()))
        {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !is_text_candidate(path) || should_ignore_path(path) {
                continue;
            }
            let Ok(source) = FileCache::read_to_string(path) else {
                continue;
            };
            for (idx, line) in source.lines().enumerate() {
                if regex.is_match(line) {
                    matches.push(GrepMatch {
                        path: path.strip_prefix(&root).unwrap_or(path).to_path_buf(),
                        line_number: idx + 1,
                        line: line.to_string(),
                    });
                    if matches.len() >= limit {
                        return Ok(matches);
                    }
                }
            }
        }

        Ok(matches)
    }

    pub fn open_file_excerpt(
        root: impl AsRef<Path>,
        path: impl AsRef<Path>,
        line_start: usize,
        context: usize,
    ) -> Result<String> {
        let root = root.as_ref().canonicalize()?;
        let abs = validate_path(&root, path.as_ref())?;
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

    pub fn git_status(root: impl AsRef<Path>) -> Result<String> {
        run_git(root.as_ref(), &["status", "--short", "--branch"])
    }

    pub fn git_diff(root: impl AsRef<Path>, range: Option<&str>) -> Result<String> {
        let mut args = vec!["diff"];
        if let Some(range) = range {
            args.push(range);
        }
        run_git(root.as_ref(), &args)
    }
}

/// Валидирует, что path находится внутри root (защита от path traversal).
pub fn validate_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let abs = candidate.canonicalize()?;

    if !abs.starts_with(&root) {
        anyhow::bail!(
            "path traversal detected: {} is outside root {}",
            abs.display(),
            root.display()
        );
    }
    Ok(abs)
}

/// Кэш файлового I/O: один и тот же файл читается с диска только раз.
pub struct FileCache;

static FILE_CACHE: once_cell::sync::Lazy<Mutex<lru::LruCache<PathBuf, String>>> =
    once_cell::sync::Lazy::new(|| {
        Mutex::new(lru::LruCache::new(
            std::num::NonZeroUsize::new(512).unwrap(),
        ))
    });

impl FileCache {
    /// Читает файл, используя кэш. Потокобезопасно.
    pub fn read_to_string(path: &Path) -> Result<String> {
        let canonical = path.canonicalize()?;
        {
            let mut cache = FILE_CACHE.lock().unwrap();
            if let Some(cached) = cache.get(&canonical) {
                return Ok(cached.clone());
            }
        }
        let content = fs::read_to_string(&canonical)?;
        {
            let mut cache = FILE_CACHE.lock().unwrap();
            cache.put(canonical, content.clone());
        }
        Ok(content)
    }

    /// Инвалидирует кэш для файла (вызывать при изменении файла).
    pub fn invalidate(path: &Path) {
        if let Ok(canonical) = path.canonicalize() {
            let mut cache = FILE_CACHE.lock().unwrap();
            cache.pop(&canonical);
        }
    }

    /// Очищает весь кэш.
    pub fn clear() {
        let mut cache = FILE_CACHE.lock().unwrap();
        cache.clear();
    }
}

fn run_git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn validate_path_rejects_parent_traversal() {
        let root = temp_dir("validate_path_rejects_parent_traversal");
        let outside = root
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("outside-{}.txt", unique_suffix()));
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, "secret").unwrap();

        let result = validate_path(
            &root,
            Path::new("..").join(outside.file_name().unwrap()).as_path(),
        );
        assert!(result.is_err());

        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validate_path_allows_inside_file() {
        let root = temp_dir("validate_path_allows_inside_file");
        fs::create_dir_all(&root).unwrap();
        let inside = root.join("inside.txt");
        fs::write(&inside, "ok").unwrap();

        let resolved = validate_path(&root, Path::new("inside.txt")).unwrap();
        assert_eq!(resolved, inside.canonicalize().unwrap());

        let _ = fs::remove_dir_all(root);
    }

    fn temp_dir(name: &str) -> PathBuf {
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
