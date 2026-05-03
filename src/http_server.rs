use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::CognitiveProjectLayer;
use crate::budget::ContextBudgetManager;
use crate::doctor;
use crate::embedding::{EmbeddingClient, EmbeddingConfig};
use crate::persistent_index::PersistentIndex;
use crate::persistent_vector::{PersistentVectorDb, build_and_save_default};
use crate::scanner::ProjectScanner;
use crate::tools::FallbackTools;

pub fn serve_project(root: impl AsRef<Path>, addr: &str) -> Result<()> {
    serve_project_with_budget(root, addr, ContextBudgetManager::default().max_tokens)
}

pub fn serve_project_with_budget(
    root: impl AsRef<Path>,
    addr: &str,
    max_tokens: usize,
) -> Result<()> {
    let root = root.as_ref().canonicalize()?;
    let listener = TcpListener::bind(addr).with_context(|| format!("failed to bind {addr}"))?;
    let layer = Arc::new(Mutex::new(CognitiveProjectLayer::initialize_with_budget(
        &root, max_tokens,
    )?));
    eprintln!("Cognitive Project Layer HTTP server");
    eprintln!("Root: {}", root.display());
    eprintln!("Context max tokens: {max_tokens}");
    eprintln!("Listening: http://{addr}");

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("http accept error: {error}");
                continue;
            }
        };
        let root = root.clone();
        let layer = Arc::clone(&layer);
        thread::spawn(move || {
            if let Err(error) = handle_stream(stream, &root, &layer) {
                eprintln!("http request error: {error}");
            }
        });
    }
    Ok(())
}

fn handle_stream(
    stream: TcpStream,
    root: &Path,
    layer: &Arc<Mutex<CognitiveProjectLayer>>,
) -> Result<()> {
    let request = read_request(&stream)?;
    let response = route_request(root, layer, request);
    write_response(stream, response)
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    body: Value,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

fn route_request(
    root: &Path,
    layer: &Arc<Mutex<CognitiveProjectLayer>>,
    request: HttpRequest,
) -> HttpResponse {
    if request.method == "OPTIONS" {
        return empty_response(204);
    }

    let result: Result<Value> = (|| match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(json!({
            "ok": true,
            "name": "cognitive-project-layer",
            "root": root,
        })),
        ("GET", "/tools") => Ok(json!({
            "tools": [
                "/health",
                "/scan",
                "/skeleton",
                "/panel",
                "/retrieve?query=...",
                "/context?query=...&max_tokens=32000",
                "/symbols?query=...",
                "/references?symbol=...",
                "/embed-search?query=...",
                "/embeddings/rebuild",
                "/index-db",
                "/index/freshness",
                "/index/rebuild",
                "/doctor",
                "/tree?depth=3",
                "/grep?pattern=..."
            ]
        })),
        ("GET", "/scan") => ProjectScanner::default()
            .scan(root)
            .map(|scan| json!({ "scan": scan, "text": scan.render_human() })),
        ("GET", "/skeleton") => with_layer(layer, |layer| {
            Ok(json!({
                "skeleton": layer.skeleton,
                "prompt": layer.skeleton.render_prompt()
            }))
        }),
        ("GET", "/panel") => {
            let query = input_string(&request, "query");
            with_layer(layer, |layer| {
                if let Some(query) = query {
                    let retrieval = layer.retrieve(&query)?;
                    Ok(json!({ "text": layer.transparency_panel(Some(&retrieval)) }))
                } else {
                    Ok(json!({ "text": layer.transparency_panel(None) }))
                }
            })
        }
        ("GET", "/retrieve") | ("POST", "/retrieve") => {
            let query = required_input(&request, "query")?;
            with_layer(layer, |layer| {
                let retrieval = layer.retrieve(&query)?;
                Ok(json!({ "retrieval": retrieval, "text": retrieval.render_human() }))
            })
        }
        ("GET", "/context") | ("POST", "/context") => {
            let query = required_input(&request, "query")?;
            let max_tokens = input_usize(&request, "max_tokens");
            with_layer(layer, |layer| {
                let retrieval = layer.retrieve(&query)?;
                let context = if let Some(max_tokens) = max_tokens {
                    ContextBudgetManager::new(max_tokens).build_context(
                        &query,
                        &layer.skeleton,
                        &layer.memory,
                        &retrieval.chunks,
                        &layer.graph,
                    )
                } else {
                    layer.build_context(&query, &retrieval)
                };
                Ok(json!({ "context": context }))
            })
        }
        ("GET", "/symbols") => {
            let query = input_string(&request, "query");
            let limit = input_usize(&request, "limit").unwrap_or(100);
            with_layer(layer, |layer| {
                let mut symbols = if let Some(query) = query {
                    layer.symbols.find(&query)
                } else {
                    layer.symbols.all()
                };
                symbols.truncate(limit);
                Ok(json!({ "symbols": symbols }))
            })
        }
        ("GET", "/references") => {
            let symbol = required_input(&request, "symbol")?;
            let limit = input_usize(&request, "limit").unwrap_or(100);
            with_layer(layer, |layer| {
                let mut references = layer.references.find(&symbol);
                references.truncate(limit);
                Ok(json!({ "references": references }))
            })
        }
        ("GET", "/embed-search") | ("POST", "/embed-search") => {
            let query = required_input(&request, "query")?;
            let limit = input_usize(&request, "limit").unwrap_or(10);
            with_layer(layer, |layer| {
                let db = layer
                    .persistent_vector_db
                    .as_ref()
                    .context("persistent vector DB not found; call /embeddings/rebuild first")?;
                let client = EmbeddingClient::new(db.config());
                let hits = db.search(&query, limit, &client)?;
                Ok(json!({ "hits": hits }))
            })
        }
        ("POST", "/embeddings/rebuild") => {
            let backend = input_string(&request, "backend").unwrap_or_else(|| "ollama".to_string());
            let model = input_string(&request, "model").or_else(|| Some("nomic-embed-text".into()));
            let dimensions = input_usize(&request, "dimensions").unwrap_or(768);
            let root = root.to_path_buf();
            with_layer(layer, |layer| {
                let config = embedding_config(&backend, model, dimensions)?;
                let (db, path) = build_and_save_default(&root, &layer.vector_store.chunks, config)?;
                layer.persistent_vector_db = Some(db.clone());
                Ok(json!({
                    "db": db,
                    "path": path,
                    "text": format!("{}\nSaved: {}", db.render_summary(), path.display())
                }))
            })
        }
        ("GET", "/vector-db") => {
            let db = PersistentVectorDb::load_default(root)?
                .context("persistent vector DB not found; call /embeddings/rebuild first")?;
            Ok(json!({ "db": db, "text": db.render_summary() }))
        }
        ("POST", "/index/rebuild") => with_layer(layer, |layer| {
            let (summary, path) = PersistentIndex::build_default(
                &layer.root,
                &layer.scan,
                &layer.symbols,
                &layer.references,
                &layer.graph,
                &layer.vector_store.chunks,
            )?;
            let text = format!("{}\nSaved: {}", summary.render_human(), path.display());
            Ok(json!({
                "summary": summary,
                "path": path,
                "text": text
            }))
        }),
        ("GET", "/index-db") => {
            let summary = PersistentIndex::summary_default(root)?
                .context("persistent SQLite index not found; call /index/rebuild first")?;
            let text = summary.render_human();
            Ok(json!({ "summary": summary, "text": text }))
        }
        ("GET", "/index/freshness") => {
            let scan = ProjectScanner::default().scan(root)?;
            let freshness = PersistentIndex::freshness_default(root, &scan)?;
            let text = freshness.render_human();
            Ok(json!({ "freshness": freshness, "text": text }))
        }
        ("GET", "/doctor") => {
            let report = doctor::run(root)?;
            let text = report.render_human();
            Ok(json!({ "report": report, "text": text }))
        }
        ("GET", "/tree") => {
            let depth = input_usize(&request, "depth").unwrap_or(3);
            FallbackTools::file_tree(root, depth).map(|text| json!({ "text": text }))
        }
        ("GET", "/grep") => {
            let pattern = required_input(&request, "pattern")?;
            let limit = input_usize(&request, "limit").unwrap_or(30);
            FallbackTools::grep(root, &pattern, limit).map(|matches| json!({ "matches": matches }))
        }
        _ => Err(anyhow::anyhow!(
            "unknown endpoint: {} {}",
            request.method,
            request.path
        )),
    })();

    match result {
        Ok(value) => json_response(200, value),
        Err(error) => json_response(
            400,
            json!({
                "ok": false,
                "error": error.to_string()
            }),
        ),
    }
}

fn with_layer(
    layer: &Arc<Mutex<CognitiveProjectLayer>>,
    action: impl FnOnce(&mut CognitiveProjectLayer) -> Result<Value>,
) -> Result<Value> {
    let mut layer = layer
        .lock()
        .map_err(|_| anyhow::anyhow!("project layer lock is poisoned"))?;
    action(&mut layer)
}

fn read_request(stream: &TcpStream) -> Result<HttpRequest> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let request_line = request_line.trim_end_matches(['\r', '\n']);
    let parts = request_line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        anyhow::bail!("invalid http request line");
    }
    let method = parts[0].to_string();
    let (path, query) = split_target(parts[1]);

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse::<usize>().unwrap_or_default();
        }
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body_bytes)?;
    }
    let body = if body_bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({}))
    };

    Ok(HttpRequest {
        method,
        path,
        query,
        body,
    })
}

fn write_response(mut stream: TcpStream, response: HttpResponse) -> Result<()> {
    let reason = match response.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)?;
    stream.flush()?;
    Ok(())
}

fn json_response(status: u16, value: Value) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "application/json; charset=utf-8",
        body: serde_json::to_vec_pretty(&value).unwrap_or_else(|_| b"{}".to_vec()),
    }
}

fn empty_response(status: u16) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "text/plain; charset=utf-8",
        body: Vec::new(),
    }
}

fn split_target(target: &str) -> (String, BTreeMap<String, String>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut params = BTreeMap::new();
    for pair in query.split('&').filter(|item| !item.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), percent_decode(value));
    }
    (percent_decode(path), params)
}

fn required_input(request: &HttpRequest, name: &str) -> Result<String> {
    input_string(request, name).with_context(|| format!("missing required input `{name}`"))
}

fn input_string(request: &HttpRequest, name: &str) -> Option<String> {
    request.query.get(name).cloned().or_else(|| {
        request
            .body
            .get(name)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn input_usize(request: &HttpRequest, name: &str) -> Option<usize> {
    request
        .query
        .get(name)
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| {
            request
                .body
                .get(name)
                .and_then(Value::as_u64)
                .map(|value| value as usize)
        })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' => {
                out.push(b' ');
                idx += 1;
            }
            b'%' if idx + 2 < bytes.len() => {
                let hex = &value[idx + 1..idx + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    idx += 3;
                } else {
                    out.push(bytes[idx]);
                    idx += 1;
                }
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_url_query() {
        let (path, query) = split_target("/retrieve?query=auth+login%20token&limit=3");
        assert_eq!(path, "/retrieve");
        assert_eq!(query.get("query").unwrap(), "auth login token");
        assert_eq!(query.get("limit").unwrap(), "3");
    }
}
