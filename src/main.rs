use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cognitive_project_layer::CognitiveProjectLayer;
use cognitive_project_layer::embedding::{EmbeddingClient, EmbeddingConfig};
use cognitive_project_layer::persistent_vector::{PersistentVectorDb, build_and_save_default};
use cognitive_project_layer::qdrant::{QdrantConfig, QdrantVectorClient};
use cognitive_project_layer::scanner::ProjectScanner;
use cognitive_project_layer::tools::FallbackTools;
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(name = "cpl")]
#[command(about = "Cognitive Project Layer for coding agents")]
#[command(version)]
struct Cli {
    #[arg(long, short = 'r', default_value = ".", global = true)]
    root: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize local CPL agent config files for this project.
    Init {
        #[arg(long, default_value = "opencode")]
        client: String,
        #[arg(long, default_value = "native")]
        server: String,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "ollama")]
        embedding_backend: String,
        #[arg(long, default_value = "nomic-embed-text")]
        embedding_model: String,
        #[arg(long, default_value_t = 768)]
        embedding_dimensions: usize,
    },
    /// Fast project scan: files, languages, configs, entry candidates, git state.
    Scan {
        #[arg(long)]
        json: bool,
    },
    /// Render the always-on project skeleton prompt.
    Skeleton {
        #[arg(long)]
        json: bool,
    },
    /// Print symbols or lookup a specific symbol name.
    Symbols {
        query: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Retrieve context for a user/coding-agent query.
    Retrieve {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Build managed agent context for a query using budget manager.
    Context {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show lazy indexer state.
    Index {
        #[arg(long)]
        json: bool,
    },
    /// Show structural graph summary or JSON graph.
    Graph {
        #[arg(long)]
        json: bool,
    },
    /// Search/list code-aware rich chunks.
    Chunks {
        query: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Build and persist embedding vector DB under .cpl/vector_db.json.
    EmbedIndex {
        #[arg(long, default_value = "local-hash")]
        backend: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long, default_value_t = 1536)]
        dimensions: usize,
        #[arg(long)]
        json: bool,
    },
    /// Search persistent embedding vector DB.
    EmbedSearch {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show persistent vector DB summary.
    VectorDb {
        #[arg(long)]
        json: bool,
    },
    /// Upsert persistent vectors into an external Qdrant collection.
    QdrantUpsert {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        collection: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long, default_value_t = 128)]
        batch_size: usize,
    },
    /// Search an external Qdrant collection using configured embeddings.
    QdrantSearch {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        collection: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Run file watcher daemon and incrementally refresh project cognition.
    Watch {
        #[arg(long, default_value_t = 500)]
        debounce_ms: u64,
    },
    /// Run local HTTP API server for agents/tools.
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 3878)]
        port: u16,
    },
    /// Render UI transparency panel; optionally include retrieval trace for query.
    Panel { query: Vec<String> },
    /// Navigate skeleton sections: entry-points, modules, public-api, configs, recent.
    Nav {
        section: String,
        filter: Option<String>,
    },
    /// Find symbol references/usages from the reference index.
    References {
        query: String,
        #[arg(long)]
        json: bool,
    },
    /// Show git status as a fallback tool.
    GitStatus,
    /// Show git diff as a fallback tool.
    GitDiff { range: Option<String> },
    /// Show an ignored-aware file tree.
    Tree {
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
    /// Grep project text with ignored folders excluded.
    Grep {
        pattern: String,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init {
            client,
            server,
            force,
            embedding_backend,
            embedding_model,
            embedding_dimensions,
        } => {
            let root = cli.root.canonicalize()?;
            let written = initialize_project_config(
                &root,
                &client,
                &server,
                force,
                &embedding_backend,
                &embedding_model,
                embedding_dimensions,
            )?;
            for path in written {
                println!("Wrote: {}", path.display());
            }
            println!("CPL project config initialized for {}", root.display());
        }
        Command::Scan { json } => {
            let scan = ProjectScanner::default()
                .scan(&cli.root)
                .with_context(|| format!("failed to scan {}", cli.root.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&scan)?);
            } else {
                println!("{}", scan.render_human());
            }
        }
        Command::Skeleton { json } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&layer.skeleton)?);
            } else {
                println!("{}", layer.skeleton.render_prompt());
            }
        }
        Command::Symbols { query, json } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let symbols = if let Some(query) = query {
                layer.symbols.find(&query)
            } else {
                layer.symbols.all()
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&symbols)?);
            } else {
                for symbol in symbols {
                    println!(
                        "{} {:?} {}:{}  {}",
                        symbol.name,
                        symbol.kind,
                        symbol.path.display(),
                        symbol.line_start,
                        symbol.signature
                    );
                }
            }
        }
        Command::Retrieve { query, json } => {
            let mut layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let query = query.join(" ");
            let result = layer.retrieve(&query)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", result.render_human());
            }
        }
        Command::Context { query, json } => {
            let mut layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let query = query.join(" ");
            let result = layer.retrieve(&query)?;
            let context = layer.build_context(&query, &result);
            if json {
                println!("{}", serde_json::to_string_pretty(&context)?);
            } else {
                println!("{}", context.text);
                println!("---");
                println!("Context tokens: {}", context.tokens);
                for section in context.sections {
                    println!(
                        "- {}: {} tokens [{}]",
                        section.name,
                        section.tokens,
                        if section.included {
                            "included"
                        } else {
                            "skipped"
                        }
                    );
                }
            }
        }
        Command::Index { json } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&layer.indexer)?);
            } else {
                println!("{}", layer.indexer.render_status());
            }
        }
        Command::Graph { json } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&layer.graph)?);
            } else {
                println!("{}", layer.graph.render_summary());
            }
        }
        Command::Chunks { query, limit, json } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            if let Some(query) = query {
                let hits = layer.vector_store.search(&query, limit);
                if json {
                    println!("{}", serde_json::to_string_pretty(&hits)?);
                } else {
                    for hit in hits {
                        println!(
                            "[{:.2}] {}:{}-{} {:?} symbols={}",
                            hit.score,
                            hit.chunk.path.display(),
                            hit.chunk.line_start,
                            hit.chunk.line_end,
                            hit.chunk.chunk_type,
                            hit.chunk.symbols.join(", ")
                        );
                    }
                }
            } else if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&layer.vector_store.chunks)?
                );
            } else {
                for chunk in layer.vector_store.chunks.iter().take(limit) {
                    println!(
                        "{}:{}-{} {:?} symbols={}",
                        chunk.path.display(),
                        chunk.line_start,
                        chunk.line_end,
                        chunk.chunk_type,
                        chunk.symbols.join(", ")
                    );
                }
                println!(
                    "shown {}/{} chunks",
                    limit.min(layer.vector_store.chunks.len()),
                    layer.vector_store.chunks.len()
                );
            }
        }
        Command::EmbedIndex {
            backend,
            model,
            endpoint,
            dimensions,
            json,
        } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let config = embedding_config_from_cli(&backend, model, endpoint, dimensions)?;
            let (db, path) =
                build_and_save_default(&layer.root, &layer.vector_store.chunks, config)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&db)?);
            } else {
                println!("{}", db.render_summary());
                println!("Saved: {}", path.display());
            }
        }
        Command::EmbedSearch { query, limit, json } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let db = layer
                .persistent_vector_db
                .as_ref()
                .context("persistent vector DB not found; run `cpl embed-index` first")?;
            let client = EmbeddingClient::new(db.config());
            let hits = db.search(&query.join(" "), limit, &client)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else {
                for hit in hits {
                    println!(
                        "[{:.2}] {}:{}-{} {:?} symbols={}",
                        hit.score,
                        hit.chunk.path.display(),
                        hit.chunk.line_start,
                        hit.chunk.line_end,
                        hit.chunk.chunk_type,
                        hit.chunk.symbols.join(", ")
                    );
                }
            }
        }
        Command::VectorDb { json } => {
            let root = cli.root.canonicalize()?;
            let db = PersistentVectorDb::load_default(&root)?
                .context("persistent vector DB not found; run `cpl embed-index` first")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&db)?);
            } else {
                println!("{}", db.render_summary());
                println!(
                    "Path: {}",
                    PersistentVectorDb::default_path(&root).display()
                );
            }
        }
        Command::QdrantUpsert {
            url,
            collection,
            api_key,
            batch_size,
        } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let db = if let Some(db) = layer.persistent_vector_db.as_ref() {
                db.clone()
            } else {
                let config = EmbeddingConfig::from_env_or_local();
                let client = EmbeddingClient::new(config);
                PersistentVectorDb::build(&layer.root, &layer.vector_store.chunks, &client)?
            };
            let qdrant_config = qdrant_config_from_cli(url, collection, api_key);
            let target_collection = qdrant_config.collection.clone();
            let target_url = qdrant_config.url.clone();
            let client = QdrantVectorClient::new(qdrant_config)?;
            let count = client.upsert_db(&db, batch_size)?;
            println!(
                "Upserted {count} vectors into Qdrant collection `{}` at {}",
                target_collection, target_url
            );
        }
        Command::QdrantSearch {
            query,
            url,
            collection,
            api_key,
            limit,
            json,
        } => {
            let query = query.join(" ");
            let config = qdrant_config_from_cli(url, collection, api_key);
            let client = QdrantVectorClient::new(config)?;
            let root = cli.root.canonicalize()?;
            let embedding_config = PersistentVectorDb::load_default(&root)?
                .map(|db| db.config())
                .unwrap_or_else(EmbeddingConfig::from_env_or_local);
            let embeddings = EmbeddingClient::new(embedding_config);
            let hits = client.search(&query, limit, &embeddings)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else {
                for hit in hits {
                    println!(
                        "[{:.2}] {}:{}-{} {:?} symbols={}",
                        hit.score,
                        hit.chunk.path.display(),
                        hit.chunk.line_start,
                        hit.chunk.line_end,
                        hit.chunk.chunk_type,
                        hit.chunk.symbols.join(", ")
                    );
                }
            }
        }
        Command::Watch { debounce_ms } => {
            cognitive_project_layer::watcher::watch_project(
                &cli.root,
                Duration::from_millis(debounce_ms),
            )?;
        }
        Command::Serve { host, port } => {
            cognitive_project_layer::http_server::serve_project(
                &cli.root,
                &format!("{host}:{port}"),
            )?;
        }
        Command::Panel { query } => {
            let mut layer = CognitiveProjectLayer::initialize(&cli.root)?;
            if query.is_empty() {
                println!("{}", layer.transparency_panel(None));
            } else {
                let query = query.join(" ");
                let result = layer.retrieve(&query)?;
                println!("{}", layer.transparency_panel(Some(&result)));
            }
        }
        Command::Nav { section, filter } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let filter = filter.unwrap_or_default().to_lowercase();
            match section.as_str() {
                "entry-points" | "entries" | "entry" => {
                    for entry in &layer.skeleton.entry_points {
                        println!(
                            "{} [{:?}] — {}",
                            entry.path.display(),
                            entry.kind,
                            entry.summary
                        );
                    }
                }
                "modules" | "module" => {
                    for module in &layer.skeleton.modules {
                        let text = module.path.to_string_lossy().to_lowercase();
                        if filter.is_empty() || text.contains(&filter) {
                            println!(
                                "{} — {} source files; key={}",
                                module.path.display(),
                                module.source_files,
                                module
                                    .key_files
                                    .iter()
                                    .map(|path| path.display().to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                    }
                }
                "public-api" | "api" | "symbols" => {
                    for api in &layer.skeleton.public_api {
                        let text =
                            format!("{} {}", api.symbol_name, api.path.display()).to_lowercase();
                        if filter.is_empty() || text.contains(&filter) {
                            println!(
                                "{} {:?} {}:{} — {}",
                                api.symbol_name,
                                api.kind,
                                api.path.display(),
                                api.line_start,
                                api.signature
                            );
                        }
                    }
                }
                "configs" | "config" => {
                    for config in &layer.skeleton.configs {
                        println!(
                            "{} [{:?}] — {}",
                            config.path.display(),
                            config.kind,
                            config.summary
                        );
                    }
                }
                "recent" | "changes" => {
                    for change in &layer.skeleton.recent_changes {
                        println!(
                            "{} [{:?}] — {}",
                            change.path.display(),
                            change.change_type,
                            change.summary
                        );
                    }
                }
                other => {
                    anyhow::bail!(
                        "unknown nav section `{}`; use entry-points, modules, public-api, configs, recent",
                        other
                    );
                }
            }
        }
        Command::References { query, json } => {
            let layer = CognitiveProjectLayer::initialize(&cli.root)?;
            let references = layer.references.find(&query);
            if json {
                println!("{}", serde_json::to_string_pretty(&references)?);
            } else {
                for reference in references {
                    println!(
                        "{}:{}:{} {:?} {}",
                        reference.path.display(),
                        reference.line_number,
                        reference.column_start,
                        reference.kind,
                        reference.snippet
                    );
                }
            }
        }
        Command::GitStatus => {
            print!("{}", FallbackTools::git_status(&cli.root)?);
        }
        Command::GitDiff { range } => {
            print!("{}", FallbackTools::git_diff(&cli.root, range.as_deref())?);
        }
        Command::Tree { depth } => {
            println!("{}", FallbackTools::file_tree(&cli.root, depth)?);
        }
        Command::Grep { pattern, limit } => {
            for item in FallbackTools::grep(&cli.root, &pattern, limit)? {
                println!(
                    "{}:{}: {}",
                    item.path.display(),
                    item.line_number,
                    item.line.trim()
                );
            }
        }
    }

    Ok(())
}

fn embedding_config_from_cli(
    backend: &str,
    model: Option<String>,
    endpoint: Option<String>,
    dimensions: usize,
) -> Result<EmbeddingConfig> {
    match backend.to_ascii_lowercase().as_str() {
        "openai" => Ok(EmbeddingConfig::openai(model)),
        "ollama" => Ok(EmbeddingConfig::ollama(model, dimensions)),
        "openai-compatible" | "local" | "local-model" => Ok(EmbeddingConfig::openai_compatible(
            endpoint.unwrap_or_else(|| "http://localhost:11434/v1/embeddings".to_string()),
            model.unwrap_or_else(|| "nomic-embed-text".to_string()),
            dimensions,
        )),
        "local-hash" | "hash" => Ok(EmbeddingConfig::local_hash(dimensions)),
        other => anyhow::bail!(
            "unknown embedding backend `{}`; use local-hash, ollama, openai, or openai-compatible",
            other
        ),
    }
}

fn qdrant_config_from_cli(
    url: Option<String>,
    collection: Option<String>,
    api_key: Option<String>,
) -> QdrantConfig {
    let mut config = QdrantConfig::from_env();
    if let Some(url) = url {
        config.url = url.trim_end_matches('/').to_string();
    }
    if let Some(collection) = collection {
        config.collection = collection;
    }
    if api_key.is_some() {
        config.api_key = api_key;
    }
    config
}

fn initialize_project_config(
    root: &Path,
    client: &str,
    server: &str,
    force: bool,
    embedding_backend: &str,
    embedding_model: &str,
    embedding_dimensions: usize,
) -> Result<Vec<PathBuf>> {
    if !client.eq_ignore_ascii_case("opencode") {
        anyhow::bail!("unsupported client `{client}`; currently supported: opencode");
    }

    let command = mcp_command_for_server(server, root)?;
    let config = opencode_config(
        root,
        command,
        embedding_backend,
        embedding_model,
        embedding_dimensions,
    );
    let opencode_path = root.join("opencode.json");
    write_json_file(&opencode_path, &config, force)?;

    let gitignore_path = root.join(".gitignore");
    ensure_gitignore_entries(
        &gitignore_path,
        &["/.cpl", ".env", ".env.*", "opencode.json"],
    )?;

    Ok(vec![opencode_path, gitignore_path])
}

fn opencode_config(
    root: &Path,
    command: Vec<String>,
    embedding_backend: &str,
    embedding_model: &str,
    embedding_dimensions: usize,
) -> Value {
    json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": {
            "cpl": {
                "type": "local",
                "command": command,
                "enabled": true,
                "timeout": 300000,
                "environment": {
                    "CPL_ROOT": portable_path(root),
                    "CPL_EMBEDDING_BACKEND": embedding_backend,
                    "CPL_EMBEDDING_MODEL": embedding_model,
                    "CPL_EMBEDDING_DIMENSIONS": embedding_dimensions.to_string()
                }
            }
        }
    })
}

fn mcp_command_for_server(server: &str, root: &Path) -> Result<Vec<String>> {
    match server.to_ascii_lowercase().as_str() {
        "native" | "rust" | "cpl-mcp" => Ok(vec![
            portable_path(&current_cpl_mcp_binary()),
            "--root".to_string(),
            portable_path(root),
        ]),
        "python" | "wrapper" | "opencode-python" => {
            let wrapper = python_wrapper_path().with_context(|| {
                "Python MCP wrapper was not found next to this CPL checkout; use `--server native`"
            })?;
            Ok(vec![
                "python".to_string(),
                portable_path(&wrapper),
                "--root".to_string(),
                portable_path(root),
            ])
        }
        other => anyhow::bail!("unknown MCP server `{other}`; use native or python"),
    }
}

fn current_cpl_mcp_binary() -> PathBuf {
    let binary_name = if cfg!(windows) {
        "cpl-mcp.exe"
    } else {
        "cpl-mcp"
    };
    let Ok(current_exe) = std::env::current_exe() else {
        return PathBuf::from(binary_name);
    };
    let sibling = current_exe.with_file_name(binary_name);
    if sibling.exists() {
        sibling
    } else {
        PathBuf::from(binary_name)
    }
}

fn python_wrapper_path() -> Option<PathBuf> {
    let manifest_path = PathBuf::from(option_env!("CARGO_MANIFEST_DIR")?);
    let wrapper = manifest_path.join("scripts").join("cpl_opencode_mcp.py");
    wrapper.exists().then_some(wrapper)
}

fn portable_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

fn write_json_file(path: &Path, value: &Value, force: bool) -> Result<()> {
    if path.exists() && !force {
        anyhow::bail!(
            "{} already exists; rerun with --force to overwrite",
            path.display()
        );
    }
    let text = format!("{}\n", serde_json::to_string_pretty(value)?);
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

fn ensure_gitignore_entries(path: &Path, entries: &[&str]) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let existing_lines = existing
        .lines()
        .map(str::trim)
        .collect::<std::collections::BTreeSet<_>>();
    let mut additions = Vec::new();
    for entry in entries {
        if !existing_lines.contains(entry) {
            additions.push(*entry);
        }
    }
    if additions.is_empty() {
        return Ok(());
    }

    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for entry in additions {
        text.push_str(entry);
        text.push('\n');
    }
    fs::write(path, text).with_context(|| format!("failed to update {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_config_contains_cpl_mcp_root_and_environment() {
        let root = PathBuf::from(r"C:\work\project");
        let config = opencode_config(
            &root,
            vec![
                "cpl-mcp".into(),
                "--root".into(),
                root.display().to_string(),
            ],
            "ollama",
            "nomic-embed-text",
            768,
        );

        assert_eq!(config["mcp"]["cpl"]["type"], "local");
        assert_eq!(config["mcp"]["cpl"]["enabled"], true);
        assert_eq!(
            config["mcp"]["cpl"]["environment"]["CPL_EMBEDDING_MODEL"],
            "nomic-embed-text"
        );
        assert!(
            config["mcp"]["cpl"]["command"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str() == Some("--root"))
        );
    }

    #[test]
    fn portable_path_strips_windows_extended_prefix() {
        assert_eq!(
            portable_path(Path::new(r"\\?\C:\work\project")),
            r"C:\work\project"
        );
        assert_eq!(
            portable_path(Path::new(r"\\?\UNC\server\share\project")),
            r"\\server\share\project"
        );
    }

    #[test]
    fn gitignore_entries_are_appended_once() {
        let root = temp_project("gitignore_entries_are_appended_once");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(".gitignore");
        fs::write(&path, "/target\n/.cpl\n").unwrap();

        ensure_gitignore_entries(&path, &["/.cpl", ".env", ".env.*", "opencode.json"]).unwrap();
        ensure_gitignore_entries(&path, &["/.cpl", ".env", ".env.*", "opencode.json"]).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("/.cpl").count(), 1);
        assert_eq!(text.matches(".env\n").count(), 1);
        assert_eq!(text.matches(".env.*").count(), 1);
        assert_eq!(text.matches("opencode.json").count(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn init_refuses_to_overwrite_existing_opencode_without_force() {
        let root = temp_project("init_refuses_to_overwrite");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("opencode.json"), "{}\n").unwrap();

        let error = initialize_project_config(
            &root,
            "opencode",
            "native",
            false,
            "local-hash",
            "hash",
            128,
        )
        .unwrap_err();

        assert!(error.to_string().contains("already exists"));
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_project(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cpl-main-{name}-{}", unique_suffix()))
    }

    fn unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }
}
