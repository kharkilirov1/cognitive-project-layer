use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::scanner::{ProjectScan, is_config_file};
use crate::symbols::{SymbolIndex, SymbolKind, SymbolLocation};

const MAX_CHUNK_LINES: usize = 120;
const CHUNK_OVERLAP_LINES: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChunkType {
    Function,
    Class,
    Struct,
    Enum,
    Interface,
    Component,
    Config,
    Test,
    Route,
    NativeBinding,
    Module,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichChunk {
    pub id: String,
    pub path: PathBuf,
    pub source: String,
    pub signature: Option<String>,
    pub docs: Option<String>,
    pub chunk_type: ChunkType,
    pub symbols: Vec<String>,
    pub imports: Vec<String>,
    pub module_path: Vec<String>,
    pub line_start: usize,
    pub line_end: usize,
}

impl RichChunk {
    pub fn chunk_project(
        root: &Path,
        scan: &ProjectScan,
        symbols: &SymbolIndex,
    ) -> Result<Vec<Self>> {
        let mut chunks = Vec::new();

        for path in &scan.source_paths {
            chunks.extend(chunk_file(root, path, symbols)?);
        }

        for rel in &scan.config_files {
            let abs = root.join(rel);
            if !abs.exists() {
                continue;
            }
            if let Ok(source) = fs::read_to_string(&abs) {
                let line_end = source.lines().count().max(1);
                chunks.push(RichChunk {
                    id: chunk_id(rel, "config", None, 1, line_end),
                    path: rel.clone(),
                    source,
                    signature: None,
                    docs: None,
                    chunk_type: ChunkType::Config,
                    symbols: Vec::new(),
                    imports: Vec::new(),
                    module_path: module_path(rel),
                    line_start: 1,
                    line_end,
                });
            }
        }

        chunks.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.line_start.cmp(&right.line_start))
        });
        Ok(chunks)
    }

    pub fn embed_text(&self) -> String {
        // Минимум boilerplate: только значимые поля, без меток-заголовков
        let mut parts = Vec::new();
        if let Some(sig) = &self.signature {
            parts.push(sig.clone());
        }
        if let Some(d) = &self.docs {
            parts.push(d.clone());
        }
        parts.extend(self.symbols.clone());
        parts.extend(self.imports.clone());
        parts.push(self.source.lines().take(40).collect::<Vec<_>>().join("\n"));
        parts.join("\n")
    }
}

pub fn chunk_file(root: &Path, abs_path: &Path, symbols: &SymbolIndex) -> Result<Vec<RichChunk>> {
    let Ok(source) = fs::read_to_string(abs_path) else {
        return Ok(Vec::new());
    };
    let rel = abs_path
        .strip_prefix(root)
        .unwrap_or(abs_path)
        .to_path_buf();
    let lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let line_count = lines.len().max(1);
    let imports = extract_imports(&source);
    let file_symbols = symbols_for_file(abs_path, symbols);

    if is_config_file(abs_path) {
        return Ok(vec![RichChunk {
            id: chunk_id(&rel, "config", None, 1, line_count),
            path: rel.clone(),
            source,
            signature: None,
            docs: None,
            chunk_type: ChunkType::Config,
            symbols: Vec::new(),
            imports,
            module_path: module_path(&rel),
            line_start: 1,
            line_end: line_count,
        }]);
    }

    if !file_symbols.is_empty() {
        let mut chunks = Vec::new();
        for symbol in &file_symbols {
            let start = symbol.line_start.max(1);
            let end = symbol.line_end.min(line_count).max(start);
            let docs = collect_docs(&lines, start);
            let ranges = chunk_ranges(start, end, MAX_CHUNK_LINES, CHUNK_OVERLAP_LINES);
            for (part_idx, (part_start, part_end)) in ranges.into_iter().enumerate() {
                chunks.push(RichChunk {
                    id: chunk_id(&rel, "symbol", Some(&symbol.name), part_start, part_end),
                    path: rel.clone(),
                    source: slice_lines(&lines, part_start, part_end),
                    signature: Some(symbol.signature.clone()),
                    docs: (part_idx == 0).then(|| docs.clone()).flatten(),
                    chunk_type: chunk_type_for_symbol(symbol, &rel),
                    symbols: vec![symbol.name.clone()],
                    imports: imports.clone(),
                    module_path: module_path(&rel),
                    line_start: part_start,
                    line_end: part_end,
                });
            }
        }
        return Ok(chunks);
    }

    let mut chunks = Vec::new();
    for (start, end) in chunk_ranges(1, line_count, MAX_CHUNK_LINES, CHUNK_OVERLAP_LINES) {
        chunks.push(RichChunk {
            id: chunk_id(&rel, "file", None, start, end),
            path: rel.clone(),
            source: slice_lines(&lines, start, end),
            signature: None,
            docs: None,
            chunk_type: infer_file_chunk_type(&rel),
            symbols: Vec::new(),
            imports: imports.clone(),
            module_path: module_path(&rel),
            line_start: start,
            line_end: end,
        });
    }

    Ok(chunks)
}

pub fn extract_imports(source: &str) -> Vec<String> {
    let patterns = [
        r#"(?m)^\s*(?:import|export)\s+.*?\s+from\s+["']([^"']+)["']"#,
        r#"(?m)^\s*import\s+["']([^"']+)["']"#,
        r#"(?m)require\(\s*["']([^"']+)["']\s*\)"#,
        r#"(?m)^\s*use\s+([A-Za-z0-9_:]+)"#,
        r#"(?m)^\s*mod\s+([A-Za-z_][A-Za-z0-9_]*)"#,
        r#"(?m)^\s*#include\s+["<]([^">]+)[">]"#,
        r#"(?m)^\s*from\s+([A-Za-z0-9_\.]+)\s+import"#,
        r#"(?m)^\s*import\s+([A-Za-z0-9_\.]+)"#,
    ];

    let mut imports = BTreeSet::new();
    for pattern in patterns {
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        for caps in regex.captures_iter(source) {
            if let Some(matched) = caps.get(1) {
                let item = matched.as_str().trim().trim_end_matches(';');
                if !item.is_empty() {
                    imports.insert(item.to_string());
                }
            }
        }
    }
    imports.into_iter().collect()
}

const CODE_STOP_WORDS: &[&str] = &[
    "fn",
    "let",
    "return",
    "pub",
    "use",
    "self",
    "mut",
    "const",
    "static",
    "if",
    "else",
    "for",
    "while",
    "match",
    "impl",
    "struct",
    "enum",
    "trait",
    "type",
    "where",
    "as",
    "in",
    "ref",
    "move",
    "async",
    "await",
    "unsafe",
    "super",
    "crate",
    "mod",
    "true",
    "false",
    "none",
    "some",
    "ok",
    "err",
    "import",
    "export",
    "default",
    "from",
    "class",
    "function",
    "var",
    "def",
    "return",
    "yield",
    "lambda",
    "pass",
    "break",
    "continue",
    "and",
    "or",
    "not",
    "is",
    "null",
    "undefined",
    "void",
];

pub fn tokenize_code_text(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '$' {
            current.push(ch.to_ascii_lowercase());
        } else {
            flush_token(&mut current, &mut tokens);
        }
    }
    flush_token(&mut current, &mut tokens);
    // Фильтр стоп-слов
    tokens.retain(|token| !CODE_STOP_WORDS.contains(&token.as_str()));
    tokens
}

fn flush_token(current: &mut String, tokens: &mut Vec<String>) {
    if current.chars().count() >= 2 {
        tokens.push(current.clone());
        for piece in split_identifier(current) {
            if piece.len() >= 2 {
                tokens.push(piece);
            }
        }
    }
    current.clear();
}

fn split_identifier(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut prev_lower = false;
    for ch in value.chars() {
        if ch == '_' || ch == '$' || ch == '-' {
            if !part.is_empty() {
                parts.push(part.clone());
                part.clear();
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !part.is_empty() {
            parts.push(part.clone());
            part.clear();
        }
        prev_lower = ch.is_lowercase();
        part.push(ch.to_ascii_lowercase());
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

fn symbols_for_file(abs_path: &Path, symbols: &SymbolIndex) -> Vec<SymbolLocation> {
    let mut file_symbols = symbols
        .symbols
        .iter()
        .filter(|symbol| same_path(&symbol.path, abs_path))
        .cloned()
        .collect::<Vec<_>>();
    file_symbols.sort_by_key(|symbol| symbol.line_start);
    file_symbols
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    // Сравнение по строковому представлению (избегаем canonicalize — системный вызов)
    let left_str = left.to_string_lossy().replace('\\', "/");
    let right_str = right.to_string_lossy().replace('\\', "/");
    left_str == right_str
}

fn slice_lines(lines: &[String], start: usize, end: usize) -> String {
    lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| {
            let line_no = idx + 1;
            (line_no >= start && line_no <= end).then_some(line.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn chunk_ranges(
    start: usize,
    end: usize,
    max_lines: usize,
    overlap_lines: usize,
) -> Vec<(usize, usize)> {
    if start == 0 || end < start {
        return Vec::new();
    }
    let max_lines = max_lines.max(1);
    let overlap_lines = overlap_lines.min(max_lines.saturating_sub(1));
    let mut ranges = Vec::new();
    let mut current = start;

    loop {
        let chunk_end = (current + max_lines - 1).min(end);
        ranges.push((current, chunk_end));
        if chunk_end >= end {
            break;
        }
        current = (chunk_end + 1)
            .saturating_sub(overlap_lines)
            .max(current + 1);
    }

    ranges
}

fn collect_docs(lines: &[String], start: usize) -> Option<String> {
    if start <= 1 {
        return None;
    }
    let mut docs = Vec::new();
    let mut idx = start.saturating_sub(2);
    loop {
        let line = lines.get(idx)?.trim();
        if line.starts_with("///")
            || line.starts_with("//!")
            || line.starts_with("//")
            || line.starts_with('*')
            || line.starts_with("/**")
            || line.starts_with('#')
        {
            docs.push(line.to_string());
        } else if line.is_empty() {
            // skip one blank line between docs and symbol
        } else {
            break;
        }

        if idx == 0 || docs.len() >= 8 {
            break;
        }
        idx -= 1;
    }
    docs.reverse();
    (!docs.is_empty()).then(|| docs.join("\n"))
}

fn chunk_type_for_symbol(symbol: &SymbolLocation, path: &Path) -> ChunkType {
    match symbol.kind {
        SymbolKind::Function | SymbolKind::Method => infer_function_chunk_type(path, &symbol.name),
        SymbolKind::Class => ChunkType::Class,
        SymbolKind::Struct => {
            if path.extension().and_then(|ext| ext.to_str()) == Some("ets") {
                ChunkType::Component
            } else {
                ChunkType::Struct
            }
        }
        SymbolKind::Enum => ChunkType::Enum,
        SymbolKind::Interface => ChunkType::Interface,
        SymbolKind::Component => ChunkType::Component,
        _ => infer_file_chunk_type(path),
    }
}

fn infer_function_chunk_type(path: &Path, symbol_name: &str) -> ChunkType {
    let text = format!(
        "{} {}",
        path.to_string_lossy().to_lowercase(),
        symbol_name.to_lowercase()
    );
    if text.contains("route") || text.contains("handler") {
        ChunkType::Route
    } else if text.contains("native") || text.contains("napi") || text.contains("ffi") {
        ChunkType::NativeBinding
    } else if text.contains("test") || text.contains("spec") {
        ChunkType::Test
    } else {
        ChunkType::Function
    }
}

fn infer_file_chunk_type(path: &Path) -> ChunkType {
    let text = path.to_string_lossy().to_lowercase();
    if text.contains("test") || text.contains("spec") {
        ChunkType::Test
    } else if text.contains("native") || text.contains("napi") || text.contains("cpp") {
        ChunkType::NativeBinding
    } else {
        ChunkType::Module
    }
}

fn module_path(path: &Path) -> Vec<String> {
    path.parent()
        .map(|parent| {
            parent
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn chunk_id(path: &Path, scope: &str, symbol: Option<&str>, start: usize, end: usize) -> String {
    let symbol = symbol
        .map(sanitize_id_part)
        .unwrap_or_else(|| "whole".to_string());
    format!(
        "{}::{scope}::{symbol}:{start}-{end}",
        path.to_string_lossy().replace('\\', "/"),
    )
}

fn sanitize_id_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '$' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_imports_from_ts_and_rust() {
        let imports = extract_imports(
            r#"
            import { A } from "./a";
            const b = require("../b");
            use crate::scanner::ProjectScan;
            mod graph;
            "#,
        );
        assert!(imports.contains(&"./a".to_string()));
        assert!(imports.contains(&"../b".to_string()));
        assert!(imports.contains(&"crate::scanner::ProjectScan".to_string()));
        assert!(imports.contains(&"graph".to_string()));
    }

    #[test]
    fn tokenizes_identifier_parts() {
        let tokens = tokenize_code_text("validateToken auth_middleware");
        assert!(tokens.contains(&"validatetoken".to_string()));
        assert!(tokens.contains(&"auth".to_string()));
        assert!(tokens.contains(&"middleware".to_string()));
    }

    #[test]
    fn chunk_ranges_overlap_large_ranges() {
        let ranges = chunk_ranges(1, 260, 120, 20);
        assert_eq!(ranges, vec![(1, 120), (101, 220), (201, 260)]);
    }

    #[test]
    fn chunks_large_symbol_with_overlap_and_stable_symbol_id() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("src")).unwrap();
        let file = root.join("src").join("large.rs");
        let mut source = String::from("pub fn huge() {\n");
        for idx in 0..250 {
            source.push_str(&format!("    let value_{idx} = {idx};\n"));
        }
        source.push_str("}\n");
        fs::write(&file, source).unwrap();

        let symbols = SymbolIndex::build(std::slice::from_ref(&file)).unwrap();
        let chunks = chunk_file(&root, &file, &symbols).unwrap();
        let huge_chunks = chunks
            .iter()
            .filter(|chunk| chunk.symbols == vec!["huge".to_string()])
            .collect::<Vec<_>>();

        assert!(huge_chunks.len() > 1);
        assert_eq!(huge_chunks[0].line_start, 1);
        assert_eq!(huge_chunks[0].line_end, 120);
        assert_eq!(huge_chunks[1].line_start, 101);
        assert!(huge_chunks[0].id.contains("src/large.rs::symbol::huge"));
        assert!(huge_chunks.last().unwrap().line_end >= 252);

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir() -> PathBuf {
        let id = format!(
            "cpl_chunk_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(id)
    }
}
