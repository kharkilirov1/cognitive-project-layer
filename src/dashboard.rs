use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde_json::{Value, json};

pub fn dashboard_html() -> &'static str {
    DASHBOARD_HTML
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

const DASHBOARD_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Cognitive Project Layer Dashboard</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #080b12;
      --panel: #101725;
      --panel-2: #141d2e;
      --text: #e8eefc;
      --muted: #8fa0bc;
      --line: #273247;
      --accent: #78a6ff;
      --accent-2: #7ee787;
      --warn: #ffd166;
      --bad: #ff7b72;
      --shadow: 0 18px 60px rgba(0,0,0,.35);
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      font: 14px/1.45 ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
      background: radial-gradient(circle at top left, #17223a 0, transparent 35rem), var(--bg);
      color: var(--text);
    }
    header {
      position: sticky;
      top: 0;
      z-index: 10;
      padding: 18px 24px;
      border-bottom: 1px solid rgba(255,255,255,.08);
      background: rgba(8, 11, 18, .82);
      backdrop-filter: blur(14px);
    }
    h1 { margin: 0; font-size: 21px; letter-spacing: .2px; }
    h2 { margin: 0 0 12px; font-size: 15px; }
    .sub { margin-top: 4px; color: var(--muted); }
    main { padding: 22px; max-width: 1440px; margin: 0 auto; }
    .grid { display: grid; gap: 14px; }
    .cards { grid-template-columns: repeat(5, minmax(150px, 1fr)); }
    .two { grid-template-columns: 1fr 1fr; }
    .three { grid-template-columns: 1fr 1fr 1fr; }
    .card, .panel {
      background: linear-gradient(180deg, rgba(255,255,255,.035), transparent), var(--panel);
      border: 1px solid var(--line);
      border-radius: 16px;
      box-shadow: var(--shadow);
    }
    .card { padding: 15px; min-height: 108px; }
    .card .label { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }
    .card .value { margin-top: 8px; font-size: 24px; font-weight: 700; }
    .card .detail { margin-top: 6px; color: var(--muted); overflow-wrap: anywhere; }
    .panel { padding: 16px; margin-top: 16px; }
    .toolbar { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
    input, select, button, textarea {
      border: 1px solid var(--line);
      border-radius: 10px;
      background: #0b111d;
      color: var(--text);
      padding: 9px 11px;
      font: inherit;
    }
    input, textarea { width: 100%; }
    button {
      cursor: pointer;
      background: linear-gradient(180deg, #234169, #182b47);
      border-color: #34547f;
      font-weight: 650;
    }
    button.secondary { background: #101827; }
    button.warn { background: linear-gradient(180deg, #594214, #32250d); border-color: #80622a; }
    button:hover { filter: brightness(1.08); }
    pre {
      white-space: pre-wrap;
      word-break: break-word;
      margin: 10px 0 0;
      padding: 12px;
      min-height: 90px;
      max-height: 460px;
      overflow: auto;
      border-radius: 12px;
      background: #060914;
      border: 1px solid #1e293d;
      color: #d7e3ff;
    }
    table { width: 100%; border-collapse: collapse; }
    th, td { text-align: left; padding: 8px 9px; border-bottom: 1px solid rgba(255,255,255,.07); vertical-align: top; }
    th { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: .06em; }
    .pill {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      padding: 3px 8px;
      border-radius: 999px;
      background: #142033;
      color: var(--muted);
      border: 1px solid var(--line);
      font-size: 12px;
    }
    .ok { color: var(--accent-2); }
    .warn-text { color: var(--warn); }
    .bad { color: var(--bad); }
    .muted { color: var(--muted); }
    .mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    .hits { display: grid; gap: 8px; margin-top: 10px; }
    .hit { padding: 10px; border: 1px solid var(--line); border-radius: 12px; background: var(--panel-2); }
    .hit strong { color: var(--accent); }
    @media (max-width: 1050px) { .cards, .two, .three { grid-template-columns: 1fr; } }
  </style>
</head>
<body>
  <header>
    <h1>Cognitive Project Layer Dashboard</h1>
    <div class="sub">Local project cognition: health, indexes, vectors, retrieval, and benchmarks.</div>
  </header>
  <main>
    <section class="grid cards" id="cards">
      <div class="card"><div class="label">Health</div><div class="value" id="healthValue">...</div><div class="detail" id="healthDetail"></div></div>
      <div class="card"><div class="label">Sources</div><div class="value" id="sourceValue">...</div><div class="detail" id="sourceDetail"></div></div>
      <div class="card"><div class="label">SQLite index</div><div class="value" id="indexValue">...</div><div class="detail" id="indexDetail"></div></div>
      <div class="card"><div class="label">Vector DB</div><div class="value" id="vectorValue">...</div><div class="detail" id="vectorDetail"></div></div>
      <div class="card"><div class="label">Doctor</div><div class="value" id="doctorValue">...</div><div class="detail" id="doctorDetail"></div></div>
    </section>

    <section class="panel">
      <h2>Maintenance</h2>
      <div class="toolbar">
        <button onclick="refreshIndex()">Refresh SQLite index</button>
        <button onclick="refreshEmbeddings()">Refresh embeddings</button>
        <button class="secondary" onclick="loadOverview()">Reload overview</button>
        <span class="pill">served by <span class="mono">cpl serve</span></span>
      </div>
      <pre id="maintenanceOut">Ready.</pre>
    </section>

    <section class="grid three">
      <div class="panel">
        <h2>Hybrid retrieve</h2>
        <div class="toolbar">
          <input id="retrieveQuery" value="persistent vector sqlite" onkeydown="submitOnEnter(event, runRetrieve)">
          <button onclick="runRetrieve()">Run</button>
        </div>
        <pre id="retrieveOut"></pre>
      </div>
      <div class="panel">
        <h2>FTS index search</h2>
        <div class="toolbar">
          <input id="indexQuery" value="dashboard vector" onkeydown="submitOnEnter(event, runIndexSearch)">
          <button onclick="runIndexSearch()">Run</button>
        </div>
        <div class="hits" id="indexHits"></div>
      </div>
      <div class="panel">
        <h2>Embedding search</h2>
        <div class="toolbar">
          <input id="embedQuery" value="lazy sqlite vector search" onkeydown="submitOnEnter(event, runEmbedSearch)">
          <button onclick="runEmbedSearch()">Run</button>
        </div>
        <div class="hits" id="embedHits"></div>
      </div>
    </section>

    <section class="grid two">
      <div class="panel">
        <h2>Transparency panel</h2>
        <div class="toolbar">
          <input id="panelQuery" value="HTTP server dashboard" onkeydown="submitOnEnter(event, runPanel)">
          <button onclick="runPanel()">Render</button>
        </div>
        <pre id="panelOut"></pre>
      </div>
      <div class="panel">
        <h2>Benchmarks / eval history</h2>
        <div class="toolbar">
          <button class="secondary" onclick="loadBenchmarks()">Reload benchmarks</button>
        </div>
        <div id="benchmarks"></div>
      </div>
    </section>

    <section class="panel">
      <h2>API tools</h2>
      <div id="tools" class="toolbar"></div>
    </section>
  </main>
  <script>
    const $ = (id) => document.getElementById(id);
    const esc = (value) => String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]));
    const ms = (value) => Number.isFinite(Number(value)) ? `${Number(value).toFixed(1)} ms` : 'n/a';

    async function api(path, options = {}) {
      const response = await fetch(path, {
        headers: { 'Content-Type': 'application/json' },
        ...options,
      });
      const type = response.headers.get('content-type') || '';
      const body = type.includes('application/json') ? await response.json() : await response.text();
      if (!response.ok || body.ok === false) throw new Error(body.error || response.statusText || body);
      return body;
    }

    async function loadOverview() {
      await Promise.allSettled([
        loadHealth(), loadScan(), loadDoctor(), loadIndexFreshness(), loadVectorDb(), loadTools(), loadBenchmarks()
      ]);
    }

    async function loadHealth() {
      try {
        const data = await api('/health');
        $('healthValue').textContent = data.ok ? 'ok' : 'warn';
        $('healthValue').className = data.ok ? 'value ok' : 'value warn-text';
        $('healthDetail').textContent = data.root || '';
      } catch (error) { failCard('health', error); }
    }

    async function loadScan() {
      try {
        const data = await api('/scan');
        const scan = data.scan || {};
        $('sourceValue').textContent = scan.source_files ?? 'n/a';
        $('sourceDetail').textContent = (scan.languages || []).join(', ');
      } catch (error) { failCard('source', error); }
    }

    async function loadDoctor() {
      try {
        const data = await api('/doctor');
        const report = data.report || {};
        const checks = report.checks || [];
        const errors = checks.filter(c => c.status === 'Error').length;
        const warnings = checks.filter(c => c.status === 'Warning').length;
        $('doctorValue').textContent = errors ? 'errors' : warnings ? 'warnings' : 'ok';
        $('doctorValue').className = 'value ' + (errors ? 'bad' : warnings ? 'warn-text' : 'ok');
        $('doctorDetail').textContent = `${checks.length} checks, version ${report.version || 'unknown'}`;
      } catch (error) { failCard('doctor', error); }
    }

    async function loadIndexFreshness() {
      try {
        const data = await api('/index/freshness');
        const f = data.freshness || {};
        $('indexValue').textContent = f.fresh ? 'fresh' : 'stale';
        $('indexValue').className = 'value ' + (f.fresh ? 'ok' : 'warn-text');
        $('indexDetail').textContent = f.reason || f.path || '';
      } catch (error) { failCard('index', error); }
    }

    async function loadVectorDb() {
      try {
        const data = await api('/vector-db');
        const db = data.db || {};
        $('vectorValue').textContent = db.records_total ?? (db.records || []).length ?? 'n/a';
        $('vectorDetail').textContent = `${db.storage || 'unknown'} · ${db.backend || ''} · ${db.model || ''}`;
      } catch (error) {
        $('vectorValue').textContent = 'missing';
        $('vectorValue').className = 'value warn-text';
        $('vectorDetail').textContent = error.message;
      }
    }

    async function loadTools() {
      try {
        const data = await api('/tools');
        $('tools').innerHTML = (data.tools || []).map(tool => `<span class="pill mono">${esc(tool)}</span>`).join('');
      } catch {}
    }

    async function loadBenchmarks() {
      const target = $('benchmarks');
      try {
        const data = await api('/benchmarks');
        const files = data.files || [];
        if (!files.length) {
          target.innerHTML = '<p class="muted">No .cpl/eval-results/*.json files found.</p>';
          return;
        }
        target.innerHTML = files.slice(0, 8).map(file => {
          const ops = (file.operations || []).slice(0, 8).map(op =>
            `<tr><td>${esc(op.operation || op.target || op.case || 'operation')}</td><td>${ms(op.p50_ms)}</td><td>${ms(op.p95_ms)}</td></tr>`
          ).join('');
          const date = file.modified_unix ? new Date(file.modified_unix * 1000).toLocaleString() : 'unknown time';
          return `<div class="hit"><strong>${esc(file.file)}</strong> <span class="pill">${esc(file.kind)}</span><div class="muted">${esc(date)}</div><table><thead><tr><th>operation</th><th>p50</th><th>p95</th></tr></thead><tbody>${ops || '<tr><td colspan="3" class="muted">No benchmark records</td></tr>'}</tbody></table></div>`;
        }).join('');
      } catch (error) {
        target.innerHTML = `<p class="bad">${esc(error.message)}</p>`;
      }
    }

    async function refreshIndex() {
      await runAction('/index/refresh', { max_incremental_files: 256 });
      await loadOverview();
    }

    async function refreshEmbeddings() {
      await runAction('/embeddings/refresh', {
        backend: 'ollama',
        model: 'nomic-embed-text',
        dimensions: 768,
        max_incremental_paths: 256,
      });
      await loadOverview();
    }

    async function runAction(path, body) {
      $('maintenanceOut').textContent = `Running ${path}...`;
      try {
        const data = await api(path, { method: 'POST', body: JSON.stringify(body) });
        $('maintenanceOut').textContent = data.text || JSON.stringify(data, null, 2);
      } catch (error) {
        $('maintenanceOut').textContent = `ERROR: ${error.message}`;
      }
    }

    async function runRetrieve() {
      const query = $('retrieveQuery').value.trim();
      $('retrieveOut').textContent = 'Running...';
      try {
        const data = await api(`/retrieve?query=${encodeURIComponent(query)}`);
        $('retrieveOut').textContent = data.text || JSON.stringify(data, null, 2);
      } catch (error) { $('retrieveOut').textContent = `ERROR: ${error.message}`; }
    }

    async function runPanel() {
      const query = $('panelQuery').value.trim();
      $('panelOut').textContent = 'Running...';
      try {
        const data = await api(`/panel?query=${encodeURIComponent(query)}`);
        $('panelOut').textContent = data.text || JSON.stringify(data, null, 2);
      } catch (error) { $('panelOut').textContent = `ERROR: ${error.message}`; }
    }

    async function runIndexSearch() {
      await runHits(`/index/search?limit=8&query=${encodeURIComponent($('indexQuery').value.trim())}`, 'indexHits', 'hits');
    }

    async function runEmbedSearch() {
      await runHits(`/embed-search?limit=8&query=${encodeURIComponent($('embedQuery').value.trim())}`, 'embedHits', 'hits');
    }

    async function runHits(path, targetId, key) {
      const target = $(targetId);
      target.innerHTML = '<div class="muted">Running...</div>';
      try {
        const data = await api(path);
        const hits = data[key] || [];
        target.innerHTML = hits.map(hit => {
          const chunk = hit.chunk || {};
          const score = hit.score || hit.rank || '';
          return `<div class="hit"><strong>${esc(chunk.path || hit.path || 'unknown')}</strong> <span class="pill">${esc(score)}</span><div class="muted">${esc((chunk.symbols || hit.symbols || []).join(', '))}</div><pre>${esc(chunk.source || hit.preview || hit.source || '')}</pre></div>`;
        }).join('') || '<div class="muted">No hits.</div>';
      } catch (error) {
        target.innerHTML = `<div class="bad">${esc(error.message)}</div>`;
      }
    }

    function failCard(prefix, error) {
      $(`${prefix}Value`).textContent = 'error';
      $(`${prefix}Value`).className = 'value bad';
      $(`${prefix}Detail`).textContent = error.message;
    }

    function submitOnEnter(event, fn) {
      if (event.key === 'Enter') fn();
    }

    loadOverview();
  </script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_html_contains_core_panels() {
        let html = dashboard_html();
        assert!(html.contains("Cognitive Project Layer Dashboard"));
        assert!(html.contains("/index/refresh"));
        assert!(html.contains("/embeddings/refresh"));
        assert!(html.contains("Benchmarks / eval history"));
    }

    #[test]
    fn benchmark_history_is_empty_when_eval_dir_is_missing() {
        let root =
            std::env::temp_dir().join(format!("cpl-dashboard-missing-{}", std::process::id()));
        let history = benchmark_history(&root).unwrap();
        assert_eq!(history["files"].as_array().unwrap().len(), 0);
    }
}
