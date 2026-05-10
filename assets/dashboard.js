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
        loadHealth(), loadScan(), loadDoctor(), loadIndexFreshness(), loadVectorDb(), loadTools(), loadBenchmarks(), loadGraph()
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

    let graphData = null;
    let graphState = { x: 0, y: 0, scale: 1, selected: null, physics: false, layoutKey: '', positioned: [], width: 0, height: 0 };
    let graphDrag = { active: false, last: null };

    async function loadGraph() {
      try {
        const data = await api('/graph');
        graphData = data.graph || { nodes: [], edges: [] };
        graphState.physics = false;
        graphState.layoutKey = '';
        renderGraph();
      } catch (error) {
        $('projectGraph').innerHTML = `<text x="16" y="28" fill="#ff7b72">${esc(error.message)}</text>`;
      }
    }

    function graphSelection() {
      const allNodes = graphData?.nodes || [];
      const allEdges = graphData?.edges || [];
      const kind = $('graphKind').value;
      const limit = Number($('graphDepth').value);
      const query = $('graphSearch').value.trim().toLowerCase();
      const degree = new Map();
      allEdges.forEach(e => { degree.set(e.from, (degree.get(e.from) || 0) + 1); degree.set(e.to, (degree.get(e.to) || 0) + 1); });
      const matches = new Set();
      const neighbors = new Set();
      if (query) {
        allNodes.forEach(n => {
          const haystack = `${n.label || ''} ${n.path || ''} ${n.id || ''}`.toLowerCase();
          if (haystack.includes(query)) matches.add(n.id);
        });
        allEdges.forEach(e => {
          if (matches.has(e.from)) neighbors.add(e.to);
          if (matches.has(e.to)) neighbors.add(e.from);
        });
      }
      let nodes = allNodes.filter(n => kind === 'all' || n.kind === kind);
      if (query) nodes = nodes.filter(n => matches.has(n.id) || neighbors.has(n.id));
      nodes = nodes.sort((a, b) => (Number(matches.has(b.id)) - Number(matches.has(a.id))) || ((degree.get(b.id) || 0) - (degree.get(a.id) || 0))).slice(0, limit);
      const ids = new Set(nodes.map(n => n.id));
      const edges = allEdges.filter(e => ids.has(e.from) && ids.has(e.to));
      return { nodes, edges, degree, matches, query, allNodes, allEdges };
    }

    function renderGraph() {
      if (!graphData) return;
      const svg = $('projectGraph');
      const { nodes, edges, degree, matches, query, allNodes, allEdges } = graphSelection();
      $('graphNodes').textContent = `${nodes.length}/${allNodes.length}`;
      $('graphEdges').textContent = `${edges.length}/${allEdges.length}`;
      renderGraphTop(nodes, degree, matches);

      const width = svg.clientWidth || 900;
      const height = svg.clientHeight || 560;
      const layoutKey = `${nodes.map(n => n.id).join('|')}::${edges.length}::${width}x${height}::${graphState.physics}`;
      if (graphState.layoutKey !== layoutKey) {
        graphState.positioned = layoutGraph(nodes, edges, width, height, degree, graphState.physics);
        graphState.layoutKey = layoutKey;
      }
      const positioned = graphState.positioned;
      const byId = new Map(positioned.map(n => [n.id, n]));
      if (graphState.selected && !byId.has(graphState.selected)) graphState.selected = null;
      const related = relatedIds(graphState.selected, edges);
      graphState.width = width;
      graphState.height = height;
      if (!positioned.length) {
        svg.innerHTML = '<foreignObject width="100%" height="100%"><div xmlns="http://www.w3.org/1999/xhtml" class="graph-empty">No graph nodes match the current filters.</div></foreignObject>';
        renderMiniMap([], [], width, height);
        $('graphDetails').textContent = 'No node selected.';
        return;
      }
      const tx = graphState.x, ty = graphState.y, sc = graphState.scale;
      const dim = graphState.selected || query;
      svg.innerHTML = `<g transform="translate(${tx} ${ty}) scale(${sc})">` +
        edges.map(e => {
          const a = byId.get(e.from), b = byId.get(e.to);
          if (!a || !b) return '';
          const active = !graphState.selected || e.from === graphState.selected || e.to === graphState.selected;
          const strokeWidth = active ? 1.8 : .8;
          const opacity = dim ? (active ? .78 : .08) : .32;
          return `<line x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" stroke="${edgeColor(e.kind)}" stroke-opacity="${opacity}" stroke-width="${strokeWidth}" />`;
        }).join('') +
        positioned.map(n => {
          const selected = graphState.selected === n.id;
          const matched = matches.has(n.id);
          const adjacent = related.has(n.id);
          const opacity = dim && !selected && !matched && !adjacent ? .28 : 1;
          const stroke = selected ? '#fff' : matched ? '#00e5ff' : adjacent ? '#b392f0' : '#0b111d';
          return `<g class="graph-node" data-id="${escAttr(n.id)}" transform="translate(${n.x.toFixed(1)} ${n.y.toFixed(1)})" opacity="${opacity}">
          <circle r="${nodeRadius(n, degree)}" fill="${nodeColor(n.kind)}" stroke="${stroke}" stroke-width="${selected || matched ? 3 : 1.5}"></circle>
          <title>${esc(n.kind)} · ${esc(n.label)}</title>
          <text x="10" y="4" fill="#d7e3ff" font-size="11" paint-order="stroke" stroke="#060914" stroke-width="3">${esc(shortLabel(n.label))}</text>
        </g>`;
        }).join('') + '</g>';
      renderMiniMap(positioned, edges, width, height);
      wireGraphEvents(positioned);
      if (graphState.selected) selectGraphNode(graphState.selected, false);
    }

    function renderGraphTop(nodes, degree, matches) {
      $('graphTop').innerHTML = nodes.slice(0, 14).map(n => `<button class="node-row ${graphState.selected === n.id ? 'active' : ''}" data-node-id="${escAttr(n.id)}"><span class="pill">${esc(n.kind)}</span> ${matches.has(n.id) ? '⌕ ' : ''}${esc(n.label)}</button>`).join('');
      $('graphTop').querySelectorAll('[data-node-id]').forEach(el => el.onclick = () => selectGraphNode(el.dataset.nodeId));
    }

    function layoutGraph(nodes, edges, width, height, degree, physics) {
      const n = nodes.length || 1;
      const rings = { Module: 0.22, Config: 0.32, File: 0.48, Symbol: 0.64 };
      const positioned = nodes.map((node, i) => {
        const angle = (i / n) * Math.PI * 2 + ((degree.get(node.id) || 0) * 0.017);
        const r = Math.min(width, height) * (rings[node.kind] || 0.55);
        return { ...node, x: width / 2 + Math.cos(angle) * r, y: height / 2 + Math.sin(angle) * r, vx: 0, vy: 0 };
      });
      if (!physics || positioned.length > 650) return positioned;
      const byId = new Map(positioned.map(node => [node.id, node]));
      const springs = edges.map(e => [byId.get(e.from), byId.get(e.to)]).filter(([a, b]) => a && b);
      const iterations = positioned.length > 360 ? 42 : 72;
      for (let step = 0; step < iterations; step++) {
        for (let i = 0; i < positioned.length; i++) {
          for (let j = i + 1; j < positioned.length; j++) {
            const a = positioned[i], b = positioned[j];
            let dx = a.x - b.x, dy = a.y - b.y;
            let dist2 = Math.max(80, dx * dx + dy * dy);
            const force = 620 / dist2;
            a.vx += dx * force; a.vy += dy * force; b.vx -= dx * force; b.vy -= dy * force;
          }
        }
        springs.forEach(([a, b]) => {
          const dx = b.x - a.x, dy = b.y - a.y;
          const dist = Math.max(1, Math.hypot(dx, dy));
          const target = a.kind === 'Symbol' || b.kind === 'Symbol' ? 58 : 92;
          const force = (dist - target) * 0.006;
          const fx = dx / dist * force, fy = dy / dist * force;
          a.vx += fx; a.vy += fy; b.vx -= fx; b.vy -= fy;
        });
        positioned.forEach(node => {
          node.vx += (width / 2 - node.x) * 0.002;
          node.vy += (height / 2 - node.y) * 0.002;
          node.x = Math.max(24, Math.min(width - 24, node.x + node.vx));
          node.y = Math.max(24, Math.min(height - 24, node.y + node.vy));
          node.vx *= 0.72; node.vy *= 0.72;
        });
      }
      return positioned;
    }

    function renderMiniMap(nodes, edges, width, height) {
      const mini = $('graphMinimap');
      const mw = mini.clientWidth || 300, mh = mini.clientHeight || 130;
      const sx = mw / Math.max(width, 1), sy = mh / Math.max(height, 1);
      const byId = new Map(nodes.map(n => [n.id, n]));
      const viewX = Math.max(0, -graphState.x / Math.max(graphState.scale, .01));
      const viewY = Math.max(0, -graphState.y / Math.max(graphState.scale, .01));
      const viewW = Math.min(width, width / Math.max(graphState.scale, .01));
      const viewH = Math.min(height, height / Math.max(graphState.scale, .01));
      mini.innerHTML = edges.slice(0, 900).map(e => {
        const a = byId.get(e.from), b = byId.get(e.to);
        return a && b ? `<line x1="${(a.x*sx).toFixed(1)}" y1="${(a.y*sy).toFixed(1)}" x2="${(b.x*sx).toFixed(1)}" y2="${(b.y*sy).toFixed(1)}" stroke="#31405d" stroke-opacity=".35" />` : '';
      }).join('') + nodes.map(n => `<circle cx="${(n.x*sx).toFixed(1)}" cy="${(n.y*sy).toFixed(1)}" r="${graphState.selected === n.id ? 3.2 : 1.8}" fill="${nodeColor(n.kind)}" opacity=".9" />`).join('') +
        `<rect x="${(viewX*sx).toFixed(1)}" y="${(viewY*sy).toFixed(1)}" width="${(viewW*sx).toFixed(1)}" height="${(viewH*sy).toFixed(1)}" fill="none" stroke="#78a6ff" stroke-width="1.5" opacity=".85" />`;
      mini.onclick = (event) => {
        const rect = mini.getBoundingClientRect();
        const targetX = ((event.clientX - rect.left) / Math.max(rect.width, 1)) * width;
        const targetY = ((event.clientY - rect.top) / Math.max(rect.height, 1)) * height;
        graphState.x = width / 2 - targetX * graphState.scale;
        graphState.y = height / 2 - targetY * graphState.scale;
        renderGraph();
      };
    }

    function wireGraphEvents(nodes) {
      const svg = $('projectGraph');
      svg.querySelectorAll('.graph-node').forEach(el => {
        el.onclick = () => selectGraphNode(el.dataset.id);
        el.onmousemove = (event) => showGraphTip(event, nodes.find(n => n.id === el.dataset.id));
        el.onmouseleave = () => $('graphTip').style.display = 'none';
      });
      svg.onpointerdown = (event) => {
        if (event.target.closest && event.target.closest('.graph-node')) return;
        graphDrag.active = true;
        graphDrag.last = { x: event.clientX, y: event.clientY };
        svg.setPointerCapture?.(event.pointerId);
      };
      svg.onpointermove = (event) => {
        if (!graphDrag.active || !graphDrag.last) return;
        graphState.x += event.clientX - graphDrag.last.x;
        graphState.y += event.clientY - graphDrag.last.y;
        graphDrag.last = { x: event.clientX, y: event.clientY };
        renderGraph();
      };
      svg.onpointerup = (event) => {
        graphDrag.active = false;
        graphDrag.last = null;
        svg.releasePointerCapture?.(event.pointerId);
      };
      svg.onpointerleave = () => { graphDrag.active = false; graphDrag.last = null; };
      svg.onwheel = (event) => {
        event.preventDefault();
        const previous = graphState.scale;
        const next = Math.max(.35, Math.min(2.8, previous * (event.deltaY > 0 ? .9 : 1.1)));
        const rect = svg.getBoundingClientRect();
        const mx = event.clientX - rect.left;
        const my = event.clientY - rect.top;
        graphState.x = mx - ((mx - graphState.x) / previous) * next;
        graphState.y = my - ((my - graphState.y) / previous) * next;
        graphState.scale = next;
        renderGraph();
      };
    }

    function selectGraphNode(id, rerender = true) {
      if (!graphData) return;
      graphState.selected = id;
      const node = (graphData.nodes || []).find(n => n.id === id);
      const allRelated = (graphData.edges || []).filter(e => e.from === id || e.to === id);
      const relationLines = allRelated.slice(0, 22).map(e => `${e.kind}: ${e.from === id ? '→ ' + e.to : '← ' + e.from}`);
      $('graphDetails').textContent = node
        ? [`${node.kind}: ${node.label}`, `path: ${node.path || ''}`, `degree: ${allRelated.length}`, '', ...relationLines].join('\n')
        : 'Node not found.';
      if (rerender) renderGraph();
    }

    function relatedIds(id, edges) {
      const out = new Set();
      if (!id) return out;
      out.add(id);
      edges.forEach(e => { if (e.from === id) out.add(e.to); if (e.to === id) out.add(e.from); });
      return out;
    }

    function focusFirstGraphMatch() {
      const { nodes, matches } = graphSelection();
      const first = nodes.find(n => matches.has(n.id)) || nodes[0];
      if (first) selectGraphNode(first.id);
    }

    function runForceLayout() {
      if ($('graphDepth').value === '9999') $('graphDepth').value = '480';
      graphState.physics = true;
      graphState.layoutKey = '';
      renderGraph();
    }

    function resetGraphView() {
      graphState.x = 0;
      graphState.y = 0;
      graphState.scale = 1;
      graphState.selected = null;
      graphState.physics = false;
      graphState.layoutKey = '';
      $('graphSearch').value = '';
      renderGraph();
    }

    function showGraphTip(event, node) {
      if (!node) return;
      const tip = $('graphTip');
      tip.innerHTML = `<strong>${esc(node.label)}</strong><div class="muted">${esc(node.kind)} · ${esc(node.path || '')}</div>`;
      tip.style.display = 'block';
      tip.style.left = `${event.offsetX + 14}px`;
      tip.style.top = `${event.offsetY + 14}px`;
    }

    function showTab(name) {
      document.querySelectorAll('.tab').forEach(tab => tab.classList.toggle('active', tab.dataset.tab === name));
      document.querySelectorAll('.tab-panel').forEach(panel => panel.hidden = panel.dataset.panel !== name);
      if (name === 'graph') setTimeout(renderGraph, 0);
    }

    function nodeColor(kind) { return ({ File: '#78a6ff', Module: '#7ee787', Symbol: '#ffd166', Config: '#ff7b72' })[kind] || '#8fa0bc'; }
    function edgeColor(kind) { return ({ Imports: '#78a6ff', Calls: '#ffd166', Contains: '#8fa0bc', InModule: '#7ee787', Tests: '#b392f0', Configures: '#ff7b72' })[kind] || '#607089'; }
    function nodeRadius(node, degree) { return Math.min(14, 5 + Math.sqrt(degree.get(node.id) || 1)); }
    function shortLabel(label) { const text = String(label || ''); return text.length > 34 ? '…' + text.slice(-33) : text; }
    function escAttr(value) { return esc(value).replace(/`/g, '&#96;'); }

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

    async function selfHeal() {
      await runAction('/heal', {
        embeddings: 'existing',
        max_incremental_files: 256,
        max_incremental_paths: 256,
      });
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
