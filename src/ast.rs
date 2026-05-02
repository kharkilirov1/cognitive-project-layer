use std::path::Path;

use anyhow::Result;
use tree_sitter::{Node, Parser};

use crate::symbols::{SymbolKind, SymbolLocation, Visibility};

pub fn parse_tree_sitter_symbols(path: &Path, source: &str) -> Result<Vec<SymbolLocation>> {
    let Some(language) = language_for_path(path) else {
        return Ok(Vec::new());
    };

    let mut parser = Parser::new();
    parser.set_language(&language)?;
    let Some(tree) = parser.parse(source, None) else {
        return Ok(Vec::new());
    };

    let mut symbols = Vec::new();
    collect_symbols(tree.root_node(), source, path, &mut symbols);
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
    Ok(symbols)
}

fn language_for_path(path: &Path) -> Option<tree_sitter::Language> {
    match path.extension().and_then(|ext| ext.to_str())? {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ts" | "ets" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "c" | "h" => {
            Some(tree_sitter_cpp::LANGUAGE.into())
        }
        _ => None,
    }
}

fn collect_symbols(node: Node<'_>, source: &str, path: &Path, symbols: &mut Vec<SymbolLocation>) {
    if let Some(kind) = symbol_kind_for_node(node)
        && let Some(name_node) = symbol_name_node(node)
    {
        let name = node_text(name_node, source).trim().to_string();
        if !name.is_empty() && !is_noise_symbol(&name) {
            let text = node_text(node, source);
            let signature = text.lines().next().unwrap_or_default().trim().to_string();
            symbols.push(SymbolLocation {
                name,
                kind,
                path: path.to_path_buf(),
                line_start: node.start_position().row + 1,
                line_end: node.end_position().row.max(node.start_position().row) + 1,
                signature,
                visibility: visibility_from_text(text),
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_symbols(child, source, path, symbols);
        }
    }
}

fn symbol_kind_for_node(node: Node<'_>) -> Option<SymbolKind> {
    match node.kind() {
        "function_item"
        | "function_declaration"
        | "function_definition"
        | "function_signature_item"
        | "function_definition_statement" => Some(SymbolKind::Function),
        "method_definition" | "method_declaration" => Some(SymbolKind::Method),
        "class_declaration" | "class" | "class_definition" | "class_specifier" => {
            Some(SymbolKind::Class)
        }
        "struct_item" | "struct_specifier" => Some(SymbolKind::Struct),
        "enum_item" | "enum_declaration" | "enum_specifier" => Some(SymbolKind::Enum),
        "interface_declaration" | "interface_type" => Some(SymbolKind::Interface),
        "trait_item" => Some(SymbolKind::Trait),
        "type_item" | "type_alias_declaration" | "type_declaration" => Some(SymbolKind::TypeAlias),
        "const_item" | "static_item" | "lexical_declaration" => Some(SymbolKind::Const),
        _ => None,
    }
}

fn symbol_name_node(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(name) = node.child_by_field_name("name") {
        return Some(name);
    }

    // Go type_declaration -> type_spec -> name.
    if node.kind() == "type_declaration" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "type_spec" {
                return child
                    .child_by_field_name("name")
                    .or_else(|| first_identifier(child));
            }
        }
    }

    // TS lexical declarations: pick the declared identifier, but only if the node looks exported.
    if node.kind() == "lexical_declaration" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "variable_declarator" {
                return child
                    .child_by_field_name("name")
                    .or_else(|| first_identifier(child));
            }
        }
    }

    first_identifier(node)
}

fn first_identifier(node: Node<'_>) -> Option<Node<'_>> {
    if matches!(
        node.kind(),
        "identifier" | "type_identifier" | "property_identifier" | "field_identifier"
    ) {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = first_identifier(child) {
            return Some(found);
        }
    }
    None
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

fn visibility_from_text(text: &str) -> Visibility {
    let first_line = text.lines().next().unwrap_or_default();
    if first_line.contains("pub ")
        || first_line.starts_with("pub(")
        || first_line.contains("export ")
    {
        Visibility::Public
    } else if first_line.contains("private ") || first_line.contains("protected ") {
        Visibility::Internal
    } else {
        Visibility::Unknown
    }
}

fn is_noise_symbol(name: &str) -> bool {
    matches!(
        name,
        "if" | "for" | "while" | "switch" | "catch" | "return" | "function"
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn parses_rust_function_with_real_range() {
        let source = r#"
pub fn validate_token(token: &str) -> bool {
    !token.is_empty()
}
"#;
        let symbols = parse_tree_sitter_symbols(Path::new("auth.rs"), source).unwrap();
        let symbol = symbols.iter().find(|s| s.name == "validate_token").unwrap();
        assert_eq!(symbol.kind, SymbolKind::Function);
        assert_eq!(symbol.line_start, 2);
        assert_eq!(symbol.line_end, 4);
        assert_eq!(symbol.visibility, Visibility::Public);
    }

    #[test]
    fn parses_typescript_class() {
        let source = "export class TdGateway { sendMessage() {} }\n";
        let symbols = parse_tree_sitter_symbols(Path::new("gateway.ts"), source).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "TdGateway" && s.kind == SymbolKind::Class)
        );
    }
}
