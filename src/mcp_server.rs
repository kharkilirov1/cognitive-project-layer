use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::budget::ContextBudgetManager;
use crate::embedding::{EmbeddingClient, EmbeddingConfig};
use crate::persistent_index::PersistentIndex;
use crate::persistent_vector::build_and_save_default;
use crate::tools::FallbackTools;
use crate::{CognitiveProjectLayer, scanner::ProjectScanner};

pub fn run_stdio(root: impl AsRef<Path>) -> Result<()> {
    run_stdio_with_budget(root, ContextBudgetManager::default().max_tokens)
}

pub fn run_stdio_with_budget(root: impl AsRef<Path>, max_tokens: usize) -> Result<()> {
    let root = root.as_ref().canonicalize()?;
    trace(&format!("start root={}", root.display()));
    let mut server = McpServer {
        root,
        layer: None,
        max_tokens,
    };
    server.run()
}

struct McpServer {
    root: PathBuf,
    layer: Option<CognitiveProjectLayer>,
    max_tokens: usize,
}

impl McpServer {
    fn run(&mut self) -> Result<()> {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();

        while let Some(message) = read_message(&mut reader)? {
            let request = serde_json::from_slice::<Value>(&message)?;
            if let Some(method) = request.get("method").and_then(Value::as_str) {
                trace(&format!("recv method={method}"));
            }
            if let Some(response) = self.handle_message(request) {
                trace("send response");
                write_message(&mut writer, &response)?;
            }
        }
        Ok(())
    }

    fn handle_message(&mut self, request: Value) -> Option<Value> {
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let id = request.get("id").cloned();

        match method {
            "initialize" => id.map(|id| {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {}
                        },
                        "serverInfo": {
                            "name": "cognitive-project-layer",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                })
            }),
            "tools/list" => id.map(|id| json_success(id, json!({ "tools": tool_definitions() }))),
            "tools/call" => id.map(|id| match self.call_tool(&request) {
                Ok(text) => json_success(
                    id,
                    json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }),
                ),
                Err(error) => json_success(
                    id,
                    json!({
                        "content": [{"type": "text", "text": error.to_string()}],
                        "isError": true
                    }),
                ),
            }),
            "ping" => id.map(|id| json_success(id, json!({}))),
            method if method.starts_with("notifications/") => None,
            _ => id.map(|id| {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("unknown MCP method `{method}`")
                    }
                })
            }),
        }
    }

    fn call_tool(&mut self, request: &Value) -> Result<String> {
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .context("tools/call params.name is required")?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        match name {
            "cpl_scan" => {
                let scan = ProjectScanner::default().scan(&self.root)?;
                Ok(scan.render_human())
            }
            "cpl_skeleton" => Ok(self.layer_mut()?.skeleton.render_prompt()),
            "cpl_panel" => Ok(self.layer_mut()?.transparency_panel(None)),
            "cpl_retrieve" => {
                let query = required_string(&args, "query")?;
                let result = self.layer_mut()?.retrieve(&query)?;
                Ok(result.render_human())
            }
            "cpl_context" => {
                let query = required_string(&args, "query")?;
                let layer = self.layer_mut()?;
                let result = layer.retrieve(&query)?;
                let context = if let Some(max_tokens) = optional_usize(&args, "max_tokens") {
                    ContextBudgetManager::new(max_tokens).build_context(
                        &query,
                        &layer.skeleton,
                        &layer.memory,
                        &result.chunks,
                        &layer.graph,
                    )
                } else {
                    layer.build_context(&query, &result)
                };
                Ok(format!(
                    "{}\n---\nContext tokens: {}\nSections: {}",
                    context.text,
                    context.tokens,
                    context
                        .sections
                        .iter()
                        .map(|section| format!(
                            "{}={}({})",
                            section.name,
                            section.tokens,
                            if section.included {
                                "included"
                            } else {
                                "skipped"
                            }
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
            "cpl_symbols" => {
                let query = optional_string(&args, "query");
                let symbols = if let Some(query) = query {
                    self.layer_mut()?.symbols.find(&query)
                } else {
                    self.layer_mut()?.symbols.all()
                };
                Ok(symbols
                    .into_iter()
                    .take(limit_arg(&args, 50))
                    .map(|symbol| {
                        format!(
                            "{} {:?} {}:{} {}",
                            symbol.name,
                            symbol.kind,
                            symbol.path.display(),
                            symbol.line_start,
                            symbol.signature
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "cpl_references" => {
                let symbol = required_string(&args, "symbol")?;
                Ok(self
                    .layer_mut()?
                    .references
                    .find(&symbol)
                    .into_iter()
                    .take(limit_arg(&args, 50))
                    .map(|reference| {
                        format!(
                            "{}:{}:{} {:?} {}",
                            reference.path.display(),
                            reference.line_number,
                            reference.column_start,
                            reference.kind,
                            reference.snippet
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "cpl_embed_search" => {
                let query = required_string(&args, "query")?;
                let db =
                    self.layer_mut()?.persistent_vector_db.as_ref().context(
                        "persistent vector DB not found; call cpl_build_embeddings first",
                    )?;
                let client = EmbeddingClient::new(db.config());
                let hits = db.search(&query, limit_arg(&args, 10), &client)?;
                Ok(hits
                    .into_iter()
                    .map(|hit| {
                        format!(
                            "[{:.2}] {}:{}-{} {:?} symbols={}",
                            hit.score,
                            hit.chunk.path.display(),
                            hit.chunk.line_start,
                            hit.chunk.line_end,
                            hit.chunk.chunk_type,
                            hit.chunk.symbols.join(", ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "cpl_build_embeddings" => {
                let backend = optional_string(&args, "backend").unwrap_or_else(|| "ollama".into());
                let model =
                    optional_string(&args, "model").or_else(|| Some("nomic-embed-text".into()));
                let dimensions = args
                    .get("dimensions")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
                    .unwrap_or(768);
                let config = embedding_config(&backend, model, dimensions)?;
                let root = self.root.clone();
                let layer = self.layer_mut()?;
                let (db, path) = build_and_save_default(&root, &layer.vector_store.chunks, config)?;
                layer.persistent_vector_db = Some(db.clone());
                Ok(format!(
                    "{}\nSaved: {}",
                    db.render_summary(),
                    path.display()
                ))
            }
            "cpl_index_build" => {
                let layer = self.layer_mut()?;
                let (summary, path) = PersistentIndex::build_default(
                    &layer.root,
                    &layer.scan,
                    &layer.symbols,
                    &layer.references,
                    &layer.graph,
                    &layer.vector_store.chunks,
                )?;
                Ok(format!(
                    "{}\nSaved: {}",
                    summary.render_human(),
                    path.display()
                ))
            }
            "cpl_index_db" => {
                let summary = PersistentIndex::summary_default(&self.root)?
                    .context("persistent SQLite index not found; call cpl_index_build first")?;
                Ok(summary.render_human())
            }
            "cpl_tree" => {
                let depth = args
                    .get("depth")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
                    .unwrap_or(3);
                FallbackTools::file_tree(&self.root, depth)
            }
            "cpl_grep" => {
                let pattern = required_string(&args, "pattern")?;
                Ok(
                    FallbackTools::grep(&self.root, &pattern, limit_arg(&args, 30))?
                        .into_iter()
                        .map(|item| {
                            format!(
                                "{}:{}: {}",
                                item.path.display(),
                                item.line_number,
                                item.line.trim()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
            _ => anyhow::bail!("unknown CPL tool `{name}`"),
        }
    }

    fn layer_mut(&mut self) -> Result<&mut CognitiveProjectLayer> {
        if self.layer.is_none() {
            self.layer = Some(CognitiveProjectLayer::initialize_with_budget(
                &self.root,
                self.max_tokens,
            )?);
        }
        Ok(self.layer.as_mut().expect("layer initialized"))
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "cpl_scan",
            "Scan the project and return languages, configs, entry points, recent changes.",
            json!({}),
        ),
        tool(
            "cpl_skeleton",
            "Return the always-on project skeleton prompt.",
            json!({}),
        ),
        tool(
            "cpl_panel",
            "Return current CPL transparency/status panel.",
            json!({}),
        ),
        tool(
            "cpl_retrieve",
            "Hybrid retrieve project context for a coding-agent query.",
            schema_query(),
        ),
        tool(
            "cpl_context",
            "Build managed LLM context for a query with skeleton, memory, retrieval and token budget.",
            schema_context_query(),
        ),
        tool(
            "cpl_symbols",
            "Find symbols by exact/fuzzy name, or list symbols if query is omitted.",
            json!({
                "query": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            }),
        ),
        tool(
            "cpl_references",
            "Find references/usages for a symbol.",
            json!({
                "symbol": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            })
            .with_required(&["symbol"]),
        ),
        tool(
            "cpl_embed_search",
            "Search the persistent local neural embedding DB.",
            json!({
                "query": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100}
            })
            .with_required(&["query"]),
        ),
        tool(
            "cpl_build_embeddings",
            "Build persistent embeddings DB. Defaults to local Ollama nomic-embed-text.",
            json!({
                "backend": {"type": "string", "enum": ["ollama", "local-hash", "openai-compatible", "openai"]},
                "model": {"type": "string"},
                "dimensions": {"type": "integer", "minimum": 8}
            }),
        ),
        tool(
            "cpl_index_build",
            "Build persistent structural SQLite index under .cpl/index.sqlite.",
            json!({}),
        ),
        tool(
            "cpl_index_db",
            "Show persistent structural SQLite index summary.",
            json!({}),
        ),
        tool(
            "cpl_tree",
            "Return ignored-aware project file tree.",
            json!({
                "depth": {"type": "integer", "minimum": 1, "maximum": 10}
            }),
        ),
        tool(
            "cpl_grep",
            "Regex/literal grep over project text, ignored folders excluded.",
            json!({
                "pattern": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            })
            .with_required(&["pattern"]),
        ),
    ]
}

fn tool(name: &str, description: &str, properties: Value) -> Value {
    let schema = if properties.get("type").is_some() {
        properties
    } else {
        json!({
            "type": "object",
            "properties": properties
        })
    };
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema
    })
}

fn schema_query() -> Value {
    json!({
        "query": {"type": "string"},
    })
    .with_required(&["query"])
}

fn schema_context_query() -> Value {
    json!({
        "query": {"type": "string"},
        "max_tokens": {"type": "integer", "minimum": 1},
    })
    .with_required(&["query"])
}

trait JsonSchemaExt {
    fn with_required(self, required: &[&str]) -> Value;
}

impl JsonSchemaExt for Value {
    fn with_required(mut self, required: &[&str]) -> Value {
        if self.get("type").is_none() {
            self = json!({
                "type": "object",
                "properties": self
            });
        }
        if let Some(object) = self.as_object_mut() {
            object.insert(
                "required".to_string(),
                Value::Array(
                    required
                        .iter()
                        .map(|item| Value::String((*item).to_string()))
                        .collect(),
                ),
            );
        }
        self
    }
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>()?);
        }
    }

    let length = content_length.context("MCP message missing Content-Length header")?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

fn write_message(writer: &mut impl Write, response: &Value) -> Result<()> {
    let body = serde_json::to_vec(response)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn json_success(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn required_string(args: &Value, name: &str) -> Result<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .with_context(|| format!("argument `{name}` is required"))
}

fn optional_string(args: &Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn limit_arg(args: &Value, default: usize) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
}

fn optional_usize(args: &Value, name: &str) -> Option<usize> {
    args.get(name)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

fn embedding_config(
    backend: &str,
    model: Option<String>,
    dimensions: usize,
) -> Result<EmbeddingConfig> {
    match backend.to_ascii_lowercase().as_str() {
        "ollama" => Ok(EmbeddingConfig::ollama(model, dimensions)),
        "local-hash" | "hash" => Ok(EmbeddingConfig::local_hash(dimensions)),
        "openai-compatible" | "local" | "local-model" => Ok(EmbeddingConfig::openai_compatible(
            std::env::var("CPL_EMBEDDING_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:11434/v1/embeddings".to_string()),
            model.unwrap_or_else(|| "nomic-embed-text".to_string()),
            dimensions,
        )),
        "openai" => Ok(EmbeddingConfig::openai(model)),
        other => anyhow::bail!("unknown embedding backend `{other}`"),
    }
}

fn trace(message: &str) {
    let Ok(path) = std::env::var("CPL_MCP_TRACE") else {
        return;
    };
    let line = format!("{:?} {message}\n", std::time::SystemTime::now());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_mcp_message() {
        let response = json!({"jsonrpc":"2.0","id":1,"result":{}});
        let mut out = Vec::new();
        write_message(&mut out, &response).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("Content-Length: "));
        assert!(text.contains("\r\n\r\n"));
    }

    #[test]
    fn tool_definitions_have_input_schemas() {
        let tools = tool_definitions();
        assert!(tools.iter().any(|tool| tool["name"] == "cpl_retrieve"));
        assert!(tools.iter().all(|tool| tool.get("inputSchema").is_some()));
        let context = tools
            .iter()
            .find(|tool| tool["name"] == "cpl_context")
            .unwrap();
        assert!(
            context["inputSchema"]["properties"]
                .get("max_tokens")
                .is_some()
        );
    }
}
