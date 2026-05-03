use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::embedding::EmbeddingConfig;
use crate::persistent_index::{PersistentIndex, PersistentIndexFreshness};
use crate::persistent_vector::PersistentVectorDb;
use crate::scanner::ProjectScanner;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DoctorStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: DoctorStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub root: PathBuf,
    pub version: String,
    pub checks: Vec<DoctorCheck>,
    pub index: Option<PersistentIndexFreshness>,
    pub vector_db_path: PathBuf,
}

impl DoctorReport {
    pub fn ok(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status == DoctorStatus::Ok)
    }

    pub fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str("CPL doctor\n");
        out.push_str(&format!("Root: {}\n", self.root.display()));
        out.push_str(&format!("Version: {}\n", self.version));
        out.push_str(&format!(
            "Overall: {}\n\n",
            if self.ok() {
                "ok"
            } else if self
                .checks
                .iter()
                .any(|check| check.status == DoctorStatus::Error)
            {
                "errors"
            } else {
                "warnings"
            }
        ));
        for check in &self.checks {
            out.push_str(&format!(
                "[{}] {} - {}\n",
                status_text(&check.status),
                check.name,
                check.message
            ));
        }
        if let Some(index) = &self.index {
            out.push('\n');
            out.push_str(&index.render_human());
        }
        out
    }
}

pub fn run(root: impl AsRef<Path>) -> Result<DoctorReport> {
    let root = root.as_ref().canonicalize()?;
    let mut checks = Vec::new();

    checks.push(DoctorCheck {
        name: "root".to_string(),
        status: DoctorStatus::Ok,
        message: root.display().to_string(),
    });

    let current_exe = std::env::current_exe().ok();
    checks.push(binary_check("cpl binary", current_exe.as_deref()));
    checks.push(cpl_mcp_check(current_exe.as_deref()));

    let scan = ProjectScanner::default().scan(&root)?;
    checks.push(DoctorCheck {
        name: "scan".to_string(),
        status: DoctorStatus::Ok,
        message: format!(
            "{} source files, languages: {}",
            scan.source_files,
            if scan.languages.is_empty() {
                "none".to_string()
            } else {
                scan.languages.join(", ")
            }
        ),
    });

    let freshness = PersistentIndex::freshness_default(&root, &scan)?;
    checks.push(DoctorCheck {
        name: "SQLite index".to_string(),
        status: if freshness.fresh {
            DoctorStatus::Ok
        } else {
            DoctorStatus::Warning
        },
        message: if freshness.fresh {
            format!("fresh at {}", freshness.path.display())
        } else {
            format!("{}; run `cpl index-build --root .`", freshness.reason)
        },
    });

    let vector_db_path = PersistentVectorDb::default_path(&root);
    match PersistentVectorDb::load_default(&root)? {
        Some(db) => checks.push(DoctorCheck {
            name: "embedding vector DB".to_string(),
            status: DoctorStatus::Ok,
            message: format!(
                "{} records, backend={}, model={}, dimensions={} at {}",
                db.record_count(),
                db.backend,
                db.model,
                db.dimensions,
                vector_db_path.display()
            ),
        }),
        None => checks.push(DoctorCheck {
            name: "embedding vector DB".to_string(),
            status: DoctorStatus::Warning,
            message: format!(
                "missing at {}; run `cpl embed-index --backend ollama --model nomic-embed-text --dimensions 768`",
                vector_db_path.display()
            ),
        }),
    }

    checks.push(ollama_check());
    checks.push(codex_mcp_config_check());

    Ok(DoctorReport {
        root,
        version: env!("CARGO_PKG_VERSION").to_string(),
        checks,
        index: Some(freshness),
        vector_db_path,
    })
}

fn binary_check(name: &str, path: Option<&Path>) -> DoctorCheck {
    match path {
        Some(path) if path.exists() => DoctorCheck {
            name: name.to_string(),
            status: DoctorStatus::Ok,
            message: path.display().to_string(),
        },
        Some(path) => DoctorCheck {
            name: name.to_string(),
            status: DoctorStatus::Warning,
            message: format!("{} does not exist", path.display()),
        },
        None => DoctorCheck {
            name: name.to_string(),
            status: DoctorStatus::Warning,
            message: "current executable path is unavailable".to_string(),
        },
    }
}

fn cpl_mcp_check(current_exe: Option<&Path>) -> DoctorCheck {
    let binary = if cfg!(windows) {
        "cpl-mcp.exe"
    } else {
        "cpl-mcp"
    };
    if let Some(current_exe) = current_exe {
        let sibling = current_exe.with_file_name(binary);
        if sibling.exists() {
            return DoctorCheck {
                name: "cpl-mcp binary".to_string(),
                status: DoctorStatus::Ok,
                message: sibling.display().to_string(),
            };
        }
    }
    if let Some(path) = find_on_path(binary) {
        return DoctorCheck {
            name: "cpl-mcp binary".to_string(),
            status: DoctorStatus::Ok,
            message: path.display().to_string(),
        };
    }
    DoctorCheck {
        name: "cpl-mcp binary".to_string(),
        status: DoctorStatus::Warning,
        message: "not found next to cpl or on PATH".to_string(),
    }
}

fn ollama_check() -> DoctorCheck {
    let config = EmbeddingConfig::from_env_or_local();
    let model = if config.model == "local-hash-embedding" {
        "nomic-embed-text".to_string()
    } else {
        config.model
    };
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return DoctorCheck {
                name: "Ollama".to_string(),
                status: DoctorStatus::Warning,
                message: format!("failed to create HTTP client: {error}"),
            };
        }
    };
    let response = match client.get("http://localhost:11434/api/tags").send() {
        Ok(response) => response,
        Err(error) => {
            return DoctorCheck {
                name: "Ollama".to_string(),
                status: DoctorStatus::Warning,
                message: format!("not reachable at localhost:11434 ({error})"),
            };
        }
    };
    if !response.status().is_success() {
        return DoctorCheck {
            name: "Ollama".to_string(),
            status: DoctorStatus::Warning,
            message: format!("/api/tags returned {}", response.status()),
        };
    }
    let value = response.json::<Value>().unwrap_or(Value::Null);
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names = models
        .iter()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let model_present = names
        .iter()
        .any(|name| *name == model || name.strip_suffix(":latest") == Some(model.as_str()));
    if model_present {
        DoctorCheck {
            name: "Ollama".to_string(),
            status: DoctorStatus::Ok,
            message: format!("reachable; model `{model}` is installed"),
        }
    } else {
        DoctorCheck {
            name: "Ollama".to_string(),
            status: DoctorStatus::Warning,
            message: format!("reachable; model `{model}` not found; run `ollama pull {model}`"),
        }
    }
}

fn codex_mcp_config_check() -> DoctorCheck {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        return DoctorCheck {
            name: "Codex MCP config".to_string(),
            status: DoctorStatus::Warning,
            message: "home directory is unavailable".to_string(),
        };
    };
    let path = PathBuf::from(home).join(".codex").join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return DoctorCheck {
            name: "Codex MCP config".to_string(),
            status: DoctorStatus::Warning,
            message: format!("{} not found", path.display()),
        };
    };
    if text.contains("[mcp_servers.cpl]") && text.contains("CPL_EMBEDDING_BACKEND") {
        DoctorCheck {
            name: "Codex MCP config".to_string(),
            status: DoctorStatus::Ok,
            message: format!("{} contains cpl MCP with embedding env", path.display()),
        }
    } else if text.contains("[mcp_servers.cpl]") {
        DoctorCheck {
            name: "Codex MCP config".to_string(),
            status: DoctorStatus::Warning,
            message: format!(
                "{} contains cpl MCP but embedding env was not detected",
                path.display()
            ),
        }
    } else {
        DoctorCheck {
            name: "Codex MCP config".to_string(),
            status: DoctorStatus::Warning,
            message: format!("{} has no [mcp_servers.cpl] block", path.display()),
        }
    }
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn status_text(status: &DoctorStatus) -> &'static str {
    match status {
        DoctorStatus::Ok => "ok",
        DoctorStatus::Warning => "warn",
        DoctorStatus::Error => "error",
    }
}

#[allow(dead_code)]
fn command_exists(binary: &str) -> bool {
    Command::new(binary)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
