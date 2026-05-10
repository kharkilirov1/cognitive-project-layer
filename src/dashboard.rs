use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde_json::{Value, json};

pub fn dashboard_html() -> &'static str {
    DASHBOARD_HTML.as_str()
}

pub fn benchmark_history(root: &Path) -> Result<Value> {
    let dir = root.join(".cpl").join("eval-results");
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(json!({
            "dir": dir,
            "files": files,
        }));
    }

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let metadata = entry.metadata()?;
        let modified_unix = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown.json")
            .to_string();
        let source = fs::read_to_string(&path)?;
        match serde_json::from_str::<Value>(&source) {
            Ok(value) => files.push(json!({
                "file": name,
                "modified_unix": modified_unix,
                "kind": result_kind(&value),
                "summary": result_summary(&value),
                "operations": operation_summaries(&value),
            })),
            Err(error) => files.push(json!({
                "file": name,
                "modified_unix": modified_unix,
                "kind": "invalid-json",
                "error": error.to_string(),
                "operations": [],
            })),
        }
    }

    files.sort_by(|left, right| {
        right
            .get("modified_unix")
            .and_then(Value::as_u64)
            .cmp(&left.get("modified_unix").and_then(Value::as_u64))
            .then_with(|| {
                left.get("file")
                    .and_then(Value::as_str)
                    .cmp(&right.get("file").and_then(Value::as_str))
            })
    });

    Ok(json!({
        "dir": dir,
        "files": files,
    }))
}

fn result_kind(value: &Value) -> &'static str {
    if value.get("records").and_then(Value::as_array).is_some() {
        "benchmark"
    } else if value.get("cases").and_then(Value::as_array).is_some()
        || value.get("summary").and_then(Value::as_object).is_some()
    {
        "eval"
    } else {
        "json"
    }
}

fn result_summary(value: &Value) -> Value {
    json!({
        "root": value.get("root"),
        "files": value.get("files"),
        "iterations": value.get("iterations"),
        "warmup": value.get("warmup"),
        "passed": value.pointer("/summary/passed").or_else(|| value.get("passed")),
        "total": value.pointer("/summary/total").or_else(|| value.get("total")),
        "avg_confidence": value.pointer("/summary/avg_confidence"),
    })
}

fn operation_summaries(value: &Value) -> Vec<Value> {
    value
        .get("records")
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    json!({
                        "operation": record.get("operation"),
                        "target": record.get("target"),
                        "case": record.get("case"),
                        "p50_ms": record.get("p50_ms"),
                        "p95_ms": record.get("p95_ms"),
                        "min_ms": record.get("min_ms"),
                        "max_ms": record.get("max_ms"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

static DASHBOARD_HTML: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
    include_str!("../assets/dashboard.html")
        .replace("{{DASHBOARD_CSS}}", include_str!("../assets/dashboard.css"))
        .replace("{{DASHBOARD_JS}}", include_str!("../assets/dashboard.js"))
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_html_contains_core_panels() {
        let html = dashboard_html();
        assert!(html.contains("Cognitive Project Layer Dashboard"));
        assert!(html.contains("/index/refresh"));
        assert!(html.contains("/embeddings/refresh"));
        assert!(html.contains("/heal"));
        assert!(html.contains("Self-heal"));
        assert!(html.contains("Benchmarks / eval history"));
        assert!(html.contains("Project graph"));
        assert!(html.contains("/graph"));
        assert!(html.contains("Force layout"));
        assert!(html.contains("graphMinimap"));
        assert!(html.contains("graphDrag"));
        assert!(html.contains("showTab('graph')"));
        assert!(html.contains("Project intelligence, visualized."));
        assert!(html.contains("zero-dependency UI"));
    }

    #[test]
    fn benchmark_history_is_empty_when_eval_dir_is_missing() {
        let root =
            std::env::temp_dir().join(format!("cpl-dashboard-missing-{}", std::process::id()));
        let history = benchmark_history(&root).unwrap();
        assert_eq!(history["files"].as_array().unwrap().len(), 0);
    }
}
