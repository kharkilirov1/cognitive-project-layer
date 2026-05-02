use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::scanner::ProjectScan;
use crate::symbols::SymbolIndex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReferenceKind {
    Identifier,
    Call,
    ComponentUse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReferenceLocation {
    pub symbol_name: String,
    pub path: PathBuf,
    pub line_number: usize,
    pub column_start: usize,
    pub kind: ReferenceKind,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReferenceIndex {
    pub by_symbol: BTreeMap<String, Vec<ReferenceLocation>>,
}

impl ReferenceIndex {
    pub fn build(root: &Path, scan: &ProjectScan, symbols: &SymbolIndex) -> Result<Self> {
        let mut index = Self::default();
        for path in &scan.source_paths {
            index.refresh_file(root, path, symbols)?;
        }
        Ok(index)
    }

    pub fn refresh_file(
        &mut self,
        root: &Path,
        abs_path: &Path,
        symbols: &SymbolIndex,
    ) -> Result<()> {
        let rel = abs_path
            .strip_prefix(root)
            .unwrap_or(abs_path)
            .to_path_buf();
        self.remove_path(&rel);

        if !abs_path.exists() {
            return Ok(());
        }

        let Ok(source) = fs::read_to_string(abs_path) else {
            return Ok(());
        };

        let known_names = symbols.by_name.keys().cloned().collect::<BTreeSet<_>>();
        if known_names.is_empty() {
            return Ok(());
        }
        let declarations = declaration_lines(root, symbols);

        for (line_idx, line) in source.lines().enumerate() {
            let line_number = line_idx + 1;
            for identifier in identifiers_in_line(line) {
                if !known_names.contains(&identifier.name) {
                    continue;
                }
                if declarations.contains(&(identifier.name.clone(), rel.clone(), line_number)) {
                    continue;
                }
                let location = ReferenceLocation {
                    symbol_name: identifier.name.clone(),
                    path: rel.clone(),
                    line_number,
                    column_start: identifier.column_start,
                    kind: identifier.kind,
                    snippet: line.trim().to_string(),
                };
                self.by_symbol
                    .entry(identifier.name)
                    .or_default()
                    .push(location);
            }
        }
        self.sort_and_dedup();
        Ok(())
    }

    pub fn find(&self, symbol_name: &str) -> Vec<ReferenceLocation> {
        self.by_symbol.get(symbol_name).cloned().unwrap_or_default()
    }

    pub fn find_any<'a>(
        &self,
        symbol_names: impl IntoIterator<Item = &'a str>,
        limit_per_symbol: usize,
    ) -> Vec<ReferenceLocation> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for name in symbol_names {
            for reference in self.find(name).into_iter().take(limit_per_symbol) {
                let key = (
                    reference.symbol_name.clone(),
                    reference.path.clone(),
                    reference.line_number,
                    reference.column_start,
                );
                if seen.insert(key) {
                    out.push(reference);
                }
            }
        }
        out.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.line_number.cmp(&right.line_number))
                .then_with(|| left.symbol_name.cmp(&right.symbol_name))
        });
        out
    }

    pub fn symbol_count(&self) -> usize {
        self.by_symbol.len()
    }

    pub fn reference_count(&self) -> usize {
        self.by_symbol.values().map(Vec::len).sum()
    }

    fn remove_path(&mut self, rel: &Path) {
        self.by_symbol.retain(|_, references| {
            references.retain(|reference| reference.path != rel);
            !references.is_empty()
        });
    }

    fn sort_and_dedup(&mut self) {
        for references in self.by_symbol.values_mut() {
            references.sort_by(|left, right| {
                left.path
                    .cmp(&right.path)
                    .then_with(|| left.line_number.cmp(&right.line_number))
                    .then_with(|| left.column_start.cmp(&right.column_start))
                    .then_with(|| format!("{:?}", left.kind).cmp(&format!("{:?}", right.kind)))
            });
            references.dedup_by(|left, right| {
                left.path == right.path
                    && left.line_number == right.line_number
                    && left.column_start == right.column_start
                    && left.kind == right.kind
            });
        }
    }
}

#[derive(Debug, Clone)]
struct IdentifierUse {
    name: String,
    column_start: usize,
    kind: ReferenceKind,
}

fn identifiers_in_line(line: &str) -> Vec<IdentifierUse> {
    let chars = line.chars().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut idx = 0usize;

    while idx < chars.len() {
        if is_ident_start(chars[idx]) {
            let start = idx;
            let mut end = idx + 1;
            while end < chars.len() && is_ident_continue(chars[end]) {
                end += 1;
            }
            let name = chars[start..end].iter().collect::<String>();
            let next = skip_whitespace(&chars, end);
            let prev = previous_non_whitespace(&chars, start);
            let kind = if prev == Some('<') {
                ReferenceKind::ComponentUse
            } else if next < chars.len() && chars[next] == '(' {
                ReferenceKind::Call
            } else {
                ReferenceKind::Identifier
            };
            out.push(IdentifierUse {
                name,
                column_start: start + 1,
                kind,
            });
            idx = end;
        } else {
            idx += 1;
        }
    }
    out
}

fn declaration_lines(root: &Path, symbols: &SymbolIndex) -> BTreeSet<(String, PathBuf, usize)> {
    symbols
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.name.clone(),
                symbol
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&symbol.path)
                    .to_path_buf(),
                symbol.line_start,
            )
        })
        .collect()
}

fn skip_whitespace(chars: &[char], mut idx: usize) -> usize {
    while idx < chars.len() && chars[idx].is_whitespace() {
        idx += 1;
    }
    idx
}

fn previous_non_whitespace(chars: &[char], idx: usize) -> Option<char> {
    chars
        .iter()
        .take(idx)
        .rev()
        .find(|ch| !ch.is_whitespace())
        .copied()
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::scanner::ProjectScanner;
    use crate::symbols::SymbolIndex;

    use super::*;

    #[test]
    fn indexes_symbol_references_without_declaration_line() {
        let root = temp_project("references");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub fn validate_token() {}\nfn caller() { validate_token(); }\n",
        )
        .unwrap();
        let scan = ProjectScanner::default().scan(&root).unwrap();
        let symbols = SymbolIndex::build(&scan.source_paths).unwrap();
        let index = ReferenceIndex::build(&root, &scan, &symbols).unwrap();
        let refs = index.find("validate_token");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].line_number, 2);
        assert_eq!(refs[0].kind, ReferenceKind::Call);
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
