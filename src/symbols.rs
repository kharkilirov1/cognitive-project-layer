use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::ast::parse_tree_sitter_symbols;
use crate::scanner::is_source_file;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Component,
    Export,
    Const,
    TypeAlias,
    Trait,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    Public,
    Internal,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolLocation {
    pub name: String,
    pub kind: SymbolKind,
    pub path: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub signature: String,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SymbolIndex {
    pub symbols: Vec<SymbolLocation>,
    pub by_name: BTreeMap<String, Vec<usize>>,
}

impl SymbolIndex {
    pub fn build(paths: &[PathBuf]) -> Result<Self> {
        let mut index = Self::default();
        for path in paths {
            index.add_file(path)?;
        }
        Ok(index)
    }

    pub fn refresh_file(&mut self, path: &Path) -> Result<()> {
        self.symbols.retain(|symbol| symbol.path != path);
        self.rebuild_map();
        if path.exists() && is_source_file(path) {
            self.add_file(path)?;
        }
        Ok(())
    }

    pub fn find(&self, query: &str) -> Vec<SymbolLocation> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        if let Some(indices) = self.by_name.get(query) {
            return indices
                .iter()
                .map(|idx| self.symbols[*idx].clone())
                .collect();
        }

        let lower = query.to_ascii_lowercase();
        let mut scored = self
            .symbols
            .iter()
            .filter_map(|symbol| {
                let symbol_lower = symbol.name.to_ascii_lowercase();
                let score = if symbol_lower == lower {
                    3
                } else if symbol_lower.contains(&lower) {
                    2
                } else if fuzzy_match(&symbol_lower, &lower) {
                    1
                } else {
                    0
                };
                (score > 0).then_some((score, symbol.clone()))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.name.cmp(&right.1.name))
                .then_with(|| left.1.path.cmp(&right.1.path))
        });
        scored.into_iter().map(|(_, symbol)| symbol).collect()
    }

    pub fn all(&self) -> Vec<SymbolLocation> {
        let mut symbols = self.symbols.clone();
        symbols.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.line_start.cmp(&right.line_start))
                .then_with(|| left.name.cmp(&right.name))
        });
        symbols
    }

    pub fn public_symbols(&self) -> Vec<SymbolLocation> {
        self.symbols
            .iter()
            .filter(|symbol| symbol.visibility == Visibility::Public)
            .cloned()
            .collect()
    }

    pub fn summary_counts(&self) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for symbol in &self.symbols {
            *counts.entry(format!("{:?}", symbol.kind)).or_insert(0) += 1;
        }
        counts
    }

    fn add_file(&mut self, path: &Path) -> Result<()> {
        let Ok(source) = fs::read_to_string(path) else {
            return Ok(());
        };
        let parsed = parse_symbols(path, &source)?;
        let start_idx = self.symbols.len();
        self.symbols.extend(parsed);
        // Инкрементальное обновление by_name
        for (offset, symbol) in self.symbols[start_idx..].iter().enumerate() {
            let idx = start_idx + offset;
            self.by_name
                .entry(symbol.name.clone())
                .or_default()
                .push(idx);
        }
        Ok(())
    }

    fn rebuild_map(&mut self) {
        self.by_name.clear();
        for (idx, symbol) in self.symbols.iter().enumerate() {
            self.by_name
                .entry(symbol.name.clone())
                .or_default()
                .push(idx);
        }
    }
}

fn parse_symbols(path: &Path, source: &str) -> Result<Vec<SymbolLocation>> {
    let language_hint = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut symbols = Vec::new();

    let mut parsed_with_tree_sitter = false;
    if let Ok(mut ast_symbols) = parse_tree_sitter_symbols(path, source) {
        parsed_with_tree_sitter = !ast_symbols.is_empty();
        symbols.append(&mut ast_symbols);
    }

    // For Rust, Tree-sitter is the primary parser. Running the regex fallback on
    // top of successful AST parsing indexes declarations embedded in test
    // fixtures/raw strings as real symbols. Keep the fallback for files that
    // Tree-sitter cannot parse or where it returns no symbols.
    if !(language_hint == "rs" && parsed_with_tree_sitter) {
        parse_rust_like(path, source, &language_hint, &mut symbols)?;
    }
    parse_ts_arkts_like(path, source, &language_hint, &mut symbols)?;
    parse_python_like(path, source, &language_hint, &mut symbols)?;
    parse_go_like(path, source, &language_hint, &mut symbols)?;

    symbols.sort_by(|left, right| {
        left.line_start
            .cmp(&right.line_start)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.signature.cmp(&right.signature))
    });
    symbols.dedup_by(|left, right| {
        left.name == right.name
            && left.kind == right.kind
            && left.line_start == right.line_start
            && left.signature == right.signature
    });
    finalize_symbol_ranges(source, &language_hint, &mut symbols);
    Ok(symbols)
}

fn parse_rust_like(
    path: &Path,
    source: &str,
    extension: &str,
    symbols: &mut Vec<SymbolLocation>,
) -> Result<()> {
    if extension != "rs" {
        return Ok(());
    }
    let patterns = [
        (
            Regex::new(r"^\s*(pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Function,
        ),
        (
            Regex::new(r"^\s*(pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Struct,
        ),
        (
            Regex::new(r"^\s*(pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Enum,
        ),
        (
            Regex::new(r"^\s*(pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Trait,
        ),
        (
            Regex::new(r"^\s*(pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Const,
        ),
        (
            Regex::new(r"^\s*(pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::TypeAlias,
        ),
    ];
    collect_by_patterns(path, source, symbols, &patterns, 1, 2);
    Ok(())
}

fn parse_ts_arkts_like(
    path: &Path,
    source: &str,
    extension: &str,
    symbols: &mut Vec<SymbolLocation>,
) -> Result<()> {
    if !matches!(
        extension,
        "ts" | "tsx" | "js" | "jsx" | "ets" | "vue" | "svelte"
    ) {
        return Ok(());
    }
    let patterns = [
        (
            Regex::new(
                r"^\s*(export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            )?,
            SymbolKind::Function,
        ),
        (
            Regex::new(r"^\s*(export\s+)?(?:default\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)")?,
            SymbolKind::Class,
        ),
        (
            Regex::new(r"^\s*(export\s+)?interface\s+([A-Za-z_$][A-Za-z0-9_$]*)")?,
            SymbolKind::Interface,
        ),
        (
            Regex::new(r"^\s*(export\s+)?enum\s+([A-Za-z_$][A-Za-z0-9_$]*)")?,
            SymbolKind::Enum,
        ),
        (
            Regex::new(r"^\s*(?:@Component\s+)?(export\s+)?struct\s+([A-Za-z_$][A-Za-z0-9_$]*)")?,
            SymbolKind::Component,
        ),
        (
            Regex::new(r"^\s*(export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)")?,
            SymbolKind::Const,
        ),
        (
            Regex::new(r"^\s*(export\s+)?type\s+([A-Za-z_$][A-Za-z0-9_$]*)")?,
            SymbolKind::TypeAlias,
        ),
        (
            Regex::new(r"^\s*(export\s+)\{([^}]+)\}")?,
            SymbolKind::Export,
        ),
    ];
    collect_by_patterns(path, source, symbols, &patterns, 1, 2);

    let method_re = Regex::new(
        r"^\s*(public\s+|private\s+|protected\s+)?(?:async\s+)?([A-Za-z_$][A-Za-z0-9_$]*)\s*\(",
    )?;
    for (idx, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("if ")
            || line.trim_start().starts_with("for ")
            || line.trim_start().starts_with("while ")
            || line.trim_start().starts_with("switch ")
        {
            continue;
        }
        if let Some(caps) = method_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            if matches!(
                name,
                "if" | "for" | "while" | "switch" | "catch" | "function"
            ) {
                continue;
            }
            let visibility = match caps.get(1).map(|m| m.as_str().trim()) {
                Some("public") => Visibility::Public,
                Some("private" | "protected") => Visibility::Internal,
                _ => Visibility::Unknown,
            };
            symbols.push(SymbolLocation {
                name: name.to_string(),
                kind: SymbolKind::Method,
                path: path.to_path_buf(),
                line_start: idx + 1,
                line_end: idx + 1,
                signature: line.trim().to_string(),
                visibility,
            });
        }
    }
    Ok(())
}

fn parse_python_like(
    path: &Path,
    source: &str,
    extension: &str,
    symbols: &mut Vec<SymbolLocation>,
) -> Result<()> {
    if extension != "py" {
        return Ok(());
    }
    let patterns = [
        (
            Regex::new(r"^\s*def\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Function,
        ),
        (
            Regex::new(r"^\s*async\s+def\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Function,
        ),
        (
            Regex::new(r"^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Class,
        ),
    ];
    collect_by_patterns(path, source, symbols, &patterns, 0, 1);
    Ok(())
}

fn parse_go_like(
    path: &Path,
    source: &str,
    extension: &str,
    symbols: &mut Vec<SymbolLocation>,
) -> Result<()> {
    if extension != "go" {
        return Ok(());
    }
    let patterns = [
        (
            Regex::new(r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)")?,
            SymbolKind::Function,
        ),
        (
            Regex::new(r"^\s*type\s+([A-Za-z_][A-Za-z0-9_]*)\s+struct")?,
            SymbolKind::Struct,
        ),
        (
            Regex::new(r"^\s*type\s+([A-Za-z_][A-Za-z0-9_]*)\s+interface")?,
            SymbolKind::Interface,
        ),
    ];
    collect_by_patterns(path, source, symbols, &patterns, 0, 1);
    Ok(())
}

fn collect_by_patterns(
    path: &Path,
    source: &str,
    symbols: &mut Vec<SymbolLocation>,
    patterns: &[(Regex, SymbolKind)],
    visibility_group: usize,
    name_group: usize,
) {
    for (idx, line) in source.lines().enumerate() {
        for (regex, kind) in patterns {
            let Some(caps) = regex.captures(line) else {
                continue;
            };
            let Some(name_match) = caps.get(name_group) else {
                continue;
            };
            let mut name = name_match.as_str().trim().to_string();
            if *kind == SymbolKind::Export && name.contains(',') {
                for part in name.split(',') {
                    let exported = part
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '$');
                    if !exported.is_empty() {
                        symbols.push(SymbolLocation {
                            name: exported.to_string(),
                            kind: kind.clone(),
                            path: path.to_path_buf(),
                            line_start: idx + 1,
                            line_end: idx + 1,
                            signature: line.trim().to_string(),
                            visibility: Visibility::Public,
                        });
                    }
                }
                continue;
            }
            name = name
                .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '$')
                .to_string();
            if name.is_empty() {
                continue;
            }
            let visibility = caps
                .get(visibility_group)
                .map(|m| m.as_str())
                .map(|raw| {
                    if raw.contains("pub") || raw.contains("export") {
                        Visibility::Public
                    } else {
                        Visibility::Unknown
                    }
                })
                .unwrap_or(Visibility::Unknown);
            symbols.push(SymbolLocation {
                name,
                kind: kind.clone(),
                path: path.to_path_buf(),
                line_start: idx + 1,
                line_end: idx + 1,
                signature: line.trim().to_string(),
                visibility,
            });
        }
    }
}

fn fuzzy_match(candidate: &str, query: &str) -> bool {
    let mut query_chars = query.chars();
    let mut current = query_chars.next();
    if current.is_none() {
        return true;
    }
    for ch in candidate.chars() {
        if Some(ch) == current {
            current = query_chars.next();
            if current.is_none() {
                return true;
            }
        }
    }
    false
}

fn finalize_symbol_ranges(source: &str, extension: &str, symbols: &mut [SymbolLocation]) {
    let lines = source.lines().collect::<Vec<_>>();
    let line_count = lines.len().max(1);

    for idx in 0..symbols.len() {
        let next_boundary = symbols
            .get(idx + 1)
            .map(|next| next.line_start.saturating_sub(1))
            .unwrap_or(line_count);
        let inferred = infer_symbol_end(&lines, symbols[idx].line_start, extension)
            .unwrap_or(next_boundary)
            .max(symbols[idx].line_start);
        symbols[idx].line_end = inferred;
    }
}

fn infer_symbol_end(lines: &[&str], line_start: usize, extension: &str) -> Option<usize> {
    if line_start == 0 || line_start > lines.len() {
        return None;
    }
    if extension == "py" {
        return infer_python_symbol_end(lines, line_start);
    }
    infer_brace_symbol_end(lines, line_start)
}

fn infer_brace_symbol_end(lines: &[&str], line_start: usize) -> Option<usize> {
    let mut balance = 0isize;
    let mut seen_open = false;

    for line_no in line_start..=lines.len() {
        let line = strip_line_comment(lines[line_no - 1]);
        for ch in line.chars() {
            match ch {
                '{' => {
                    balance += 1;
                    seen_open = true;
                }
                '}' if seen_open => {
                    balance -= 1;
                    if balance <= 0 {
                        return Some(line_no);
                    }
                }
                _ => {}
            }
        }

        if !seen_open && line.contains(';') {
            return Some(line_no);
        }

        if !seen_open && line_no.saturating_sub(line_start) > 20 {
            return Some(line_start);
        }
    }

    if seen_open {
        Some(lines.len())
    } else {
        Some(line_start)
    }
}

fn infer_python_symbol_end(lines: &[&str], line_start: usize) -> Option<usize> {
    let start_line = lines[line_start - 1];
    let start_indent = indentation(start_line);
    let mut end = line_start;

    for line_no in (line_start + 1)..=lines.len() {
        let line = lines[line_no - 1];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            end = line_no;
            continue;
        }
        if indentation(line) <= start_indent {
            break;
        }
        end = line_no;
    }

    Some(end.max(line_start))
}

fn strip_line_comment(line: &str) -> &str {
    line.split("//").next().unwrap_or(line)
}

fn indentation(line: &str) -> usize {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rust_public_function() {
        let symbols = parse_symbols(
            Path::new("src/lib.rs"),
            "pub async fn authenticate_user(token: &str) {\n  Ok(())\n}\nstruct Internal;",
        )
        .unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "authenticate_user" && s.visibility == Visibility::Public)
        );
        let authenticate = symbols
            .iter()
            .find(|s| s.name == "authenticate_user")
            .unwrap();
        assert_eq!(authenticate.line_start, 1);
        assert_eq!(authenticate.line_end, 3);
        assert!(symbols.iter().any(|s| s.name == "Internal"));
    }

    #[test]
    fn parses_arkts_component() {
        let symbols = parse_symbols(
            Path::new("Index.ets"),
            "@Component\nexport struct TgChatRow {\n  build() {}\n}\n",
        )
        .unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "TgChatRow" && s.kind == SymbolKind::Component)
        );
        let component = symbols.iter().find(|s| s.name == "TgChatRow").unwrap();
        assert_eq!(component.line_start, 2);
        assert_eq!(component.line_end, 4);
    }

    #[test]
    fn rust_parser_does_not_index_raw_string_fixture_contents() {
        let symbols = parse_symbols(
            Path::new("src/lib.rs"),
            r##"
pub fn real_api() {}

#[cfg(test)]
mod tests {
    #[test]
    fn fixture() {
        let source = r#"
pub fn fake_inside_fixture() {}
"#;
        assert!(source.contains("fake_inside_fixture"));
    }
}
"##,
        )
        .unwrap();

        assert!(symbols.iter().any(|s| s.name == "real_api"));
        assert!(
            !symbols.iter().any(|s| s.name == "fake_inside_fixture"),
            "raw string fixture contents must not become project symbols"
        );
    }
}
