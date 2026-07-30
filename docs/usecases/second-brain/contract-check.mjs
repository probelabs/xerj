// Second-brain live contract check: runs the REAL UX data layer
// (xerj-ux/src/data/second-brain-api.js) and the REAL panel renderers
// (xerj-ux/src/ux/ego-ledger.js) in Node against a LIVE brain server —
// field-for-field, whatever the dashboard dereferences must exist and behave.
//
// Usage (see REPRODUCE.md):
//   XERJ_URL=http://localhost:9340 XERJ_BRAIN=docs \
//   XERJ_ADMIN_KEY_FILE=$DATA/admin.key node contract-check.mjs
//
// This proves the rendering logic and the data contract. It does NOT prove
// real-browser pixels or pointer interactions.
import { readFileSync } from 'node:fs';

const BASE = process.env.XERJ_URL || 'http://localhost:9340';
const BRAIN = process.env.XERJ_BRAIN || 'docs';
const KEY = process.env.XERJ_ADMIN_KEY
  || (process.env.XERJ_ADMIN_KEY_FILE
    ? readFileSync(process.env.XERJ_ADMIN_KEY_FILE, 'utf8').trim()
    : null);

globalThis.location = { hash: `#/second-brain?brain=${BRAIN}` };
const rawFetch = globalThis.fetch;
globalThis.fetch = (url, opts = {}) =>
  rawFetch(url, {
    ...opts,
    headers: { ...(opts.headers || {}), ...(KEY ? { authorization: `ApiKey ${KEY}` } : {}) },
  });

const { liveSecondBrain } = await import(new URL('../../../xerj-ux/src/data/second-brain-api.js', import.meta.url));
const { renderPanelBody } = await import(new URL('../../../xerj-ux/src/ux/ego-ledger.js', import.meta.url));

let fails = 0;
const ok = (c, m) => { if (c) console.log(`  ok  ${m}`); else { fails++; console.log(`  FAIL ${m}`); } };

let d = await liveSecondBrain(BASE, {}, undefined);
const live0 = d.overview.edges.live;
const inv0 = d.overview.edges.invalidated;
ok(d.sb.connected === true, 'connected to real brain server');
ok(d.brain === BRAIN, `brain discovered from -edges suffix = ${d.brain}`);
ok(Number.isInteger(d.overview.edges.total) && d.overview.edges.total > 0, `overview.edges.total = ${d.overview.edges.total}`);
ok(d.overview.embedder === 'lexical-feature-hash', 'embedder honesty marker present');
ok(typeof d.overview.nodes_index === 'string' && d.overview.nodes_index.length > 0, `nodes_index = ${String(d.overview.nodes_index).slice(0, 60)}…`);
ok(Array.isArray(d.overview.types) && d.overview.types.every((t) => 'type' in t && 'live' in t), 'types[] shape {type,live}');
ok(Array.isArray(d.overview.detectors) && d.overview.detectors.every((t) => 'detector' in t && 'live' in t), 'detectors[] shape {detector,live}');
ok(d.overview.hubs && Array.isArray(d.overview.hubs.in) && Array.isArray(d.overview.hubs.out), 'hubs.in/out arrays');
ok(Array.isArray(d.overview.created_over_time) && d.overview.created_over_time.every((b) => 't' in b && 'count' in b), 'created_over_time[] {t,count}');
ok(d.overview.not_shown && 'hubs_out_not_listed' in d.overview.not_shown, 'overview.not_shown accounting');
ok(d.sb.focus, `default focus chosen = ${d.sb.focus}`);
ok(d.ego && Array.isArray(d.ego.edges) && d.ego.edges.length > 0, `belief fetched (${d.ego && d.ego.edges && d.ego.edges.length} links)`);
const e0 = d.ego.edges[0];
for (const f of ['edge_id', 'src', 'dst', 'type', 'weight', 'hop', 'direction', 'valid_at', 'created_at', 'detector', 'confidence']) {
  ok(f in e0, `edge field present: ${f}`);
}
ok('invalid_at' in e0, 'edge field present: invalid_at (null when live)');
ok(d.ego.nodes && typeof d.ego.nodes === 'object', 'ego.nodes map present (include_nodes)');
const n0 = Object.values(d.ego.nodes)[0];
ok(n0 && 'title' in n0 && 'preview' in n0, 'node hydration fields {title,preview}');
ok(d.ego.not_shown && 'edges_clipped' in d.ego.not_shown && 'dangling_nodes' in d.ego.not_shown, 'ego.not_shown accounting');
ok(Array.isArray(d.ego.neighbors) && (d.ego.neighbors.length === 0 || ('via_edge' in d.ego.neighbors[0])), 'neighbors[] {id,hop,via_edge}');
ok(d.sb.timeline && d.sb.timeline.edges.length > 0, `timeline fetched (${d.sb.timeline && d.sb.timeline.edges.length} edges)`);
ok(Object.keys(d.sb.names).length > 0, `names hydrated (${Object.keys(d.sb.names).length})`);

const IDS = ['edgesLive', 'edgesTotal', 'invalidated', 'detectors', 'typeDist', 'edgeTimeline', 'ego', 'hubs', 'notShown'];
let all = '';
for (const id of IDS) {
  let html = '', threw = null;
  try { html = renderPanelBody(id, d); } catch (e) { threw = e; }
  ok(!threw && html.length > 0, `render ${id}${threw ? ` THREW ${threw.stack}` : ''}`);
  all += html;
}
ok(all.includes('recall is lexical (feature hashing) — not neural'), 'embedder note rendered');
ok(all.includes('data-sb-scrub'), 'scrubber rendered');
const banned = ['valid_at', 'as_of', 'edge_id'].filter((w) => {
  const visible = all.replace(/<[^>]*>/g, ' ').replace(/data-[a-z-]+="[^"]*"/g, ' ');
  return visible.includes(w);
});
ok(banned.length === 0, `LANGUAGE rule on real data (leaked: ${banned.join(',') || 'none'})`);

// retire a real link, replay before/after, then re-assert to converge back
const focus = d.sb.focus;
const anEdge = d.ego.edges.find((e) => e.hop === 1 && e.invalid_at == null);
const before = Date.now() - 5000;
const del = await fetch(`${BASE}/_graph/${BRAIN}/link/${anEdge.edge_id}`, { method: 'DELETE' });
const delBody = await del.json();
ok(del.ok && delBody.invalidated === true, `retired real link ${anEdge.edge_id.slice(0, 8)}… (${anEdge.detector})`);

globalThis.location.hash = `#/second-brain?brain=${BRAIN}&focus=${focus}&as_of=${before}`;
d = await liveSecondBrain(BASE, {}, undefined);
ok(d.overview.edges.live === live0, `time-travel: ${live0} believed before retirement (got ${d.overview.edges.live})`);

globalThis.location.hash = `#/second-brain?brain=${BRAIN}&focus=${focus}&as_of=${Date.now()}`;
d = await liveSecondBrain(BASE, {}, undefined);
ok(d.overview.edges.live === live0 - 1 && d.overview.edges.invalidated === inv0 + 1, `now: one fewer believed, one more retired (${d.overview.edges.live}/${d.overview.edges.invalidated})`);
ok(renderPanelBody('ego', d).includes('sb-expired'), 'retired link drawn struck-through');
ok(renderPanelBody('invalidated', d).includes('retired'), 'RETIRED panel shows churn');

const re = await fetch(`${BASE}/_graph/${BRAIN}/link`, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ src: anEdge.src, dst: anEdge.dst, type: anEdge.type, weight: anEdge.weight, detector: anEdge.detector, confidence: anEdge.confidence, evidence: anEdge.evidence }),
});
console.log(`  info re-assert after retire: HTTP ${re.status}`);

console.log(fails === 0 ? '\nALL LIVE CONTRACT CHECKS PASSED' : `\n${fails} CHECK(S) FAILED`);
process.exit(fails ? 1 : 0);
