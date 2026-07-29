// ============================================================
// Xerj Console — SECOND BRAIN data layer + interaction controller
//
// This dashboard reads the LIVE graph endpoints ONLY:
//
//   GET /_cat/indices/.xerj-memory-*-edges   → discover brains
//   GET /_graph/{brain}/overview             → §4.4 of the contract
//   GET /_graph/{brain}/ego                  → §4.3 of the contract
//
// There is deliberately NO mock fallback: a fake brain would defeat
// the product ("what does my agent believe?" answered with invented
// beliefs). Every failure path returns a SHAPED object — never null —
// so data/query.js can never silently substitute mock data; the
// dashboard renders an honest unreachable / empty state instead.
//
// The second half of this file is the interaction controller. The app
// shell re-renders pages as whole HTML strings, so ledger interactions
// (refocus, belief-time scrub, evidence reveal) are handled here with
// document-level delegated listeners + in-place re-render of only the
// second-brain panel bodies (`[data-sb-body]`), each drawn by
// ux/ego-ledger.js#renderPanelBody. Scrub previews are client-side
// over already-fetched edges; RELEASING the caret re-queries the
// server with `as_of` — the server owns the bi-temporal cut.
// ============================================================

import {
  renderPanelBody, drawLedgerTraces, edgeStateAt, fmtTs,
} from '../ux/ego-ledger.js';

/** Panel ids of the second-brain dashboard, in render order. The
 *  controller patches exactly these after an interaction. */
export const SB_PANEL_IDS = [
  'edgesLive', 'edgesTotal', 'invalidated', 'detectors',
  'typeDist', 'edgeTimeline', 'ego', 'hubs', 'notShown',
];

/** Returned edge budget for the belief query. Hops=2 so neighbour rows
 *  can carry their onward-degree count; 1000 is the ego endpoint's
 *  clamp ceiling. Clips are surfaced by the honesty panel + '≥' counts. */
const EGO_LIMIT = 1000;

const K = {
  brain: 'xerj.sb.brain',
  perBrain: (b) => `xerj.sb.state.${b}`,
};

// ----- module state -------------------------------------------------
//
// A singleton, on purpose: the console is a single-page app and the
// ledger must survive the shell's whole-page re-renders (theme flips,
// time-range clicks) without losing focus/as-of. Persisted so a reload
// reopens the same view.

const S = {
  baseUrl: '',
  connected: false,
  error: null,     // transport-level error (engine unreachable)
  uiError: null,   // last interaction-level error (shown inline, non-fatal)
  brains: [],
  brain: null,
  focus: null,
  asOf: null,      // epoch-ms | null = NOW
  trail: [],
  overview: null,  // §4.4 body (or exists:false body)
  ego: null,       // §4.3 body — the belief at the caret
  timeline: null,  // §4.3 body — hop-1, include_expired, pinned at NOW
};

function loadBrainState() {
  try {
    const raw = sessionStorage.getItem(K.perBrain(S.brain));
    const st = raw ? JSON.parse(raw) : {};
    S.focus = st.focus || null;
    S.asOf = Number.isFinite(st.asOf) ? st.asOf : null;
    S.trail = Array.isArray(st.trail) ? st.trail : [];
  } catch {
    S.focus = null; S.asOf = null; S.trail = [];
  }
}

function saveBrainState() {
  try {
    if (!S.brain) return;
    sessionStorage.setItem(
      K.perBrain(S.brain),
      JSON.stringify({ focus: S.focus, asOf: S.asOf, trail: S.trail.slice(-8) }),
    );
  } catch { /* storage full/blocked — view still works, just not sticky */ }
}

// ----- HTTP ---------------------------------------------------------

async function getJson(url, signal) {
  const r = await fetch(url, { signal, headers: { accept: 'application/json' } });
  const text = await r.text();
  let body = null;
  try { body = text ? JSON.parse(text) : null; } catch { /* non-JSON error page */ }
  if (!r.ok) {
    // The graph API 404s an unknown brain WITH a shaped body
    // (`exists: false`) — that is a valid answer, not an error.
    if (body && body.exists === false) return body;
    const reason = body && body.error && body.error.reason
      ? body.error.reason
      : `HTTP ${r.status}`;
    throw new Error(reason);
  }
  return body;
}

/**
 * Brains = the reserved `.xerj-memory-{brain}-edges` indices. `_cat`
 * on this engine emits plain text (no format=json), so parse lines;
 * tolerate a JSON array in case a later build adds it. A wildcard
 * matching nothing is an empty 200 — an engine with no brains is a
 * normal state, not an error.
 */
async function discoverBrains(baseUrl, signal) {
  const r = await fetch(`${baseUrl}/_cat/indices/.xerj-memory-*-edges`, { signal });
  if (r.status === 404) return [];
  if (!r.ok) throw new Error(`_cat/indices HTTP ${r.status}`);
  const text = await r.text();
  let names = [];
  const trimmed = text.trim();
  if (trimmed.startsWith('[')) {
    try { names = JSON.parse(trimmed).map((i) => i.index).filter(Boolean); } catch { names = []; }
  } else if (trimmed) {
    // cat line: `health status NAME uuid pri rep docs deleted size size`
    names = trimmed.split('\n')
      .map((l) => l.trim().split(/\s+/)[2])
      .filter(Boolean);
  }
  return names
    .filter((n) => n.startsWith('.xerj-memory-') && n.endsWith('-edges'))
    .map((n) => n.slice('.xerj-memory-'.length, -'-edges'.length))
    .filter((b) => b.length > 0)
    .sort();
}

const enc = encodeURIComponent;

function fetchOverview(signal) {
  const qs = new URLSearchParams({ top: '10', histogram_interval: 'day' });
  if (S.asOf != null) qs.set('as_of', String(S.asOf));
  return getJson(`${S.baseUrl}/_graph/${enc(S.brain)}/overview?${qs}`, signal);
}

/**
 * The belief at the caret. `include_expired=true` so an invalidated
 * fact comes back struck-through + EXPIRED instead of silently gone;
 * hops=2 feeds the neighbour onward-counts; include_nodes hydrates
 * titles/previews for the focus card and rows.
 */
function fetchBelief(signal) {
  const qs = new URLSearchParams({
    node: S.focus, hops: '2', direction: 'both',
    limit: String(EGO_LIMIT),
    include_expired: 'true', include_nodes: 'true', include_evidence: 'true',
  });
  if (S.asOf != null) qs.set('as_of', String(S.asOf));
  return getJson(`${S.baseUrl}/_graph/${enc(S.brain)}/ego?${qs}`, signal);
}

/**
 * The scrubber's interval set: hop-1, include_expired, pinned at NOW.
 * Pinned so scrubbing left never removes rows from the strip — only
 * their visibility state changes. Refetched on focus change only.
 */
function fetchTimeline(signal) {
  const qs = new URLSearchParams({
    node: S.focus, hops: '1', direction: 'both',
    limit: String(EGO_LIMIT),
    include_expired: 'true', include_evidence: 'false',
  });
  return getJson(`${S.baseUrl}/_graph/${enc(S.brain)}/ego?${qs}`, signal);
}

// ----- data assembly (§7.3 shape, extras under `sb`) ---------------

function assemble() {
  const o = S.overview && S.overview.exists !== false ? S.overview : null;
  const ed = (o && o.edges) || { total: 0, live: 0, invalidated: 0 };
  const num = (v) => new Intl.NumberFormat('en').format(v || 0);
  const detectors = (o && o.detectors) || [];
  const detTail = o && o.not_shown && o.not_shown.detectors_not_listed > 0 ? '+' : '';
  return {
    brain: S.brain,
    overview: S.overview,
    ego: S.ego,
    metrics: {
      edgesLive: { formatted: num(ed.live), hint: 'visible at as-of' },
      edgesTotal: { formatted: num(ed.total), hint: 'incl. invalidated' },
      invalidated: { formatted: num(ed.invalidated), hint: 'soft-deleted, kept for time travel' },
      detectors: { formatted: num(detectors.length) + detTail, hint: 'deterministic, versioned' },
    },
    series: {
      created: ((o && o.created_over_time) || []).map((b) => b.count),
      createdT: ((o && o.created_over_time) || []).map((b) => b.t),
    },
    sb: {
      connected: S.connected,
      error: S.error,
      uiError: S.uiError,
      brains: S.brains.slice(),
      focus: S.focus,
      asOf: S.asOf,
      trail: S.trail.slice(),
      timeline: S.timeline,
    },
  };
}

/** Default focus: the strongest hub. Deterministic (out over in, then
 *  the agg's own count-desc order). */
function defaultFocus() {
  const o = S.overview;
  if (!o || o.exists === false || !o.hubs) return null;
  const out = o.hubs.out && o.hubs.out[0];
  const inn = o.hubs.in && o.hubs.in[0];
  return (out && out.id) || (inn && inn.id) || null;
}

// ----- backend adapter entry (data/backends/xerj.js dispatches here) —

export async function liveSecondBrain(baseUrl, _ctx, signal) {
  S.baseUrl = (baseUrl || '').replace(/\/+$/, '');
  S.uiError = null;
  try {
    S.brains = await discoverBrains(S.baseUrl, signal);
    S.connected = true;
    S.error = null;
  } catch (e) {
    S.connected = false;
    S.error = String((e && e.message) || e);
    S.brains = []; S.brain = null; S.overview = null; S.ego = null; S.timeline = null;
    const data = assemble();
    // Surface transport failure on the nav status pill too (query.js
    // flags `data.error` as live-error — visibly NOT "live · ok").
    data.error = `graph API unreachable: ${S.error}`;
    return data;
  }

  let persisted = null;
  try { persisted = localStorage.getItem(K.brain); } catch { /* private mode */ }
  S.brain = persisted && S.brains.includes(persisted) ? persisted : (S.brains[0] || null);
  if (!S.brain) {
    S.overview = null; S.ego = null; S.timeline = null;
    return assemble();
  }
  loadBrainState();

  try {
    S.overview = await fetchOverview(signal);
  } catch (e) {
    S.overview = null; S.ego = null; S.timeline = null;
    const data = assemble();
    data.error = `overview failed: ${String((e && e.message) || e)}`;
    return data;
  }

  const hasEdges = S.overview && S.overview.exists !== false
    && S.overview.edges && S.overview.edges.total > 0;
  if (!hasEdges) {
    S.ego = null; S.timeline = null;
    return assemble();
  }

  if (!S.focus) S.focus = defaultFocus();
  if (S.focus) {
    try {
      const [belief, timeline] = await Promise.all([
        fetchBelief(signal), fetchTimeline(signal),
      ]);
      S.ego = belief;
      S.timeline = timeline;
    } catch (e) {
      S.ego = null; S.timeline = null;
      S.uiError = String((e && e.message) || e);
    }
  } else {
    S.ego = null; S.timeline = null;
  }
  saveBrainState();
  return assemble();
}

// ====================================================================
// Interaction controller
// ====================================================================

let inflight = null; // AbortController of the interaction in flight

function abortInflight() {
  if (inflight) { inflight.abort(); inflight = null; }
}

/** Re-render every second-brain panel body currently in the DOM from
 *  module state, then redraw the traces post-paint. */
function patchPanels() {
  if (typeof document === 'undefined') return;
  const data = assemble();
  for (const id of SB_PANEL_IDS) {
    const el = document.querySelector(`[data-sb-body="${id}"]`);
    if (el) el.innerHTML = renderPanelBody(id, data);
  }
  showUiError();
  scheduleTraces();
}

function showUiError() {
  const slot = document.querySelector('[data-sb-error]');
  if (!slot) return;
  slot.textContent = S.uiError ? `QUERY FAILED · ${String(S.uiError).slice(0, 90)}` : '';
}

function setBusy(on) {
  const el = document.querySelector('[data-sb-body="ego"]');
  if (el) el.classList.toggle('sb-busy', !!on);
}

let traceRaf = 0;
function scheduleTraces() {
  if (typeof requestAnimationFrame === 'undefined') return;
  if (traceRaf) cancelAnimationFrame(traceRaf);
  traceRaf = requestAnimationFrame(() => {
    traceRaf = 0;
    drawLedgerTraces(document);
  });
}

/**
 * Post-render hook. The dashboard's render() calls this every time the
 * shell rebuilds the page so the trace overlay (which needs real
 * layout) is drawn after paint. Safe under Node (no-op) — the render
 * functions themselves stay pure.
 */
export function sbAfterRender() {
  scheduleTraces();
}

// ----- interaction ops ----------------------------------------------

async function refetch({ overview = false, belief = false, timeline = false }) {
  abortInflight();
  inflight = typeof AbortController !== 'undefined' ? new AbortController() : null;
  const signal = inflight && inflight.signal;
  setBusy(true);
  S.uiError = null;
  try {
    const jobs = [];
    if (overview) jobs.push(fetchOverview(signal).then((r) => { S.overview = r; }));
    if (belief && S.focus) jobs.push(fetchBelief(signal).then((r) => { S.ego = r; }));
    if (timeline && S.focus) jobs.push(fetchTimeline(signal).then((r) => { S.timeline = r; }));
    await Promise.all(jobs);
  } catch (e) {
    if (!(e && e.name === 'AbortError')) {
      S.uiError = String((e && e.message) || e);
    }
  } finally {
    setBusy(false);
  }
  saveBrainState();
  patchPanels();
}

function refocus(id) {
  if (!id || id === S.focus) return;
  if (S.focus) {
    S.trail = S.trail.filter((t) => t !== S.focus);
    S.trail.push(S.focus);
    S.trail = S.trail.slice(-8);
  }
  S.focus = id;
  S.ego = null; S.timeline = null;
  patchPanels(); // immediate: focus card swaps to the new node id
  refetch({ belief: true, timeline: true });
}

function switchBrain(name) {
  if (!name || name === S.brain) return;
  try { localStorage.setItem(K.brain, name); } catch { /* private mode */ }
  S.brain = name;
  S.overview = null; S.ego = null; S.timeline = null;
  loadBrainState();
  patchPanels();
  (async () => {
    await refetch({ overview: true });
    if (!S.focus) S.focus = defaultFocus();
    if (S.focus) await refetch({ belief: true, timeline: true });
  })();
}

/** Commit an as-of. `null` = NOW. Overview + belief re-query (both are
 *  as-of-dependent); the timeline stays pinned at NOW by design. */
function commitAsOf(ms) {
  S.asOf = ms;
  refetch({ overview: true, belief: true });
}

// ----- belief-time scrub (preview client-side, commit server-side) ---

function stripDomain(scrub) {
  const d0 = Number(scrub.getAttribute('data-d0'));
  const d1 = Number(scrub.getAttribute('data-d1'));
  return Number.isFinite(d0) && Number.isFinite(d1) && d1 > d0 ? [d0, d1] : null;
}

function msFromClientX(strip, dom, clientX) {
  const rect = strip.getBoundingClientRect();
  const frac = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
  return Math.round(dom[0] + frac * (dom[1] - dom[0]));
}

/** ≥99% of the way to the right edge counts as NOW (as_of unset). */
function normalizeAsOf(ms, dom) {
  return (dom[1] - ms) / (dom[1] - dom[0]) < 0.01 ? null : ms;
}

/** Move the caret + relabel + restate every strip line and ledger row
 *  for a candidate as-of. Pure DOM, no queries — this is the mid-drag
 *  preview over edges the server already returned. */
function previewAsOf(scrub, ms) {
  const dom = stripDomain(scrub);
  if (!dom) return;
  const pct = ((ms - dom[0]) / (dom[1] - dom[0])) * 100;
  const caret = scrub.querySelector('[data-sb-caret]');
  const label = scrub.querySelector('[data-sb-caret-label]');
  const atNow = normalizeAsOf(ms, dom) == null;
  if (caret) caret.style.left = `${pct}%`;
  if (label) {
    label.style.left = `${pct}%`;
    label.textContent = atNow ? 'NOW' : fmtTs(ms);
  }
  const t = atNow ? null : ms;
  for (const seg of scrub.querySelectorAll('.sb-lseg')) {
    const e = { valid_at: Number(seg.getAttribute('data-va')), invalid_at: seg.hasAttribute('data-vi') ? Number(seg.getAttribute('data-vi')) : null };
    const state = edgeStateAt(e, t);
    seg.classList.remove('sb-l-live', 'sb-l-future', 'sb-l-expired');
    seg.classList.add(`sb-l-${state}`);
  }
  for (const row of document.querySelectorAll('[data-sb-edge][data-va]')) {
    const e = { valid_at: Number(row.getAttribute('data-va')), invalid_at: row.hasAttribute('data-vi') ? Number(row.getAttribute('data-vi')) : null };
    const state = edgeStateAt(e, t);
    row.classList.toggle('sb-off', state === 'future');
    row.classList.toggle('sb-expired', state === 'expired');
  }
}

// ----- event wiring (once per page load) ----------------------------

let wired = false;
let drag = null; // { scrub, dom, ms }

function onPointerDown(e) {
  const caret = e.target.closest && e.target.closest('[data-sb-caret]');
  if (!caret) return;
  const scrub = caret.closest('[data-sb-scrub]');
  const dom = scrub && stripDomain(scrub);
  if (!dom) return;
  e.preventDefault();
  drag = { scrub, dom, ms: S.asOf == null ? dom[1] : S.asOf };
  if (caret.setPointerCapture && e.pointerId != null) {
    try { caret.setPointerCapture(e.pointerId); } catch { /* older engines */ }
  }
}

function onPointerMove(e) {
  if (!drag) return;
  const strip = drag.scrub.querySelector('[data-sb-strip]');
  if (!strip) { drag = null; return; }
  drag.ms = msFromClientX(strip, drag.dom, e.clientX);
  previewAsOf(drag.scrub, drag.ms);
}

function onPointerUp() {
  if (!drag) return;
  const ms = normalizeAsOf(drag.ms, drag.dom);
  drag = null;
  commitAsOf(ms);
}

function onClick(e) {
  const t = e.target;
  if (!t || !t.closest) return;

  const brainBtn = t.closest('[data-sb-brain]');
  if (brainBtn) { switchBrain(brainBtn.getAttribute('data-sb-brain')); return; }

  if (t.closest('[data-sb-now]')) { commitAsOf(null); return; }

  const focusBtn = t.closest('[data-sb-focus]');
  if (focusBtn) { refocus(focusBtn.getAttribute('data-sb-focus')); return; }

  const evBtn = t.closest('[data-sb-evidence]');
  if (evBtn) {
    const row = evBtn.closest('[data-sb-edge]');
    const body = row && row.querySelector('[data-sb-evbody]');
    if (body) body.hidden = !body.hidden;
    return;
  }

  // Click on the strip (not the caret) = jump the caret there.
  const strip = t.closest('[data-sb-strip]');
  if (strip && !t.closest('[data-sb-caret]')) {
    const scrub = strip.closest('[data-sb-scrub]');
    const dom = scrub && stripDomain(scrub);
    if (dom) {
      const ms = msFromClientX(strip, dom, e.clientX);
      previewAsOf(scrub, ms);
      commitAsOf(normalizeAsOf(ms, dom));
    }
  }
}

let keyCommit = 0;
function onKeyDown(e) {
  const caret = e.target && e.target.closest && e.target.closest('[data-sb-caret]');
  if (!caret) return;
  if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
  const scrub = caret.closest('[data-sb-scrub]');
  const dom = scrub && stripDomain(scrub);
  if (!dom) return;
  e.preventDefault();
  const step = Math.max(60_000, Math.round((dom[1] - dom[0]) / 100));
  const cur = S.asOf == null ? dom[1] : S.asOf;
  const next = Math.max(dom[0], Math.min(dom[1], cur + (e.key === 'ArrowRight' ? step : -step)));
  previewAsOf(scrub, next);
  // Debounce the server commit so holding the key doesn't queue queries.
  clearTimeout(keyCommit);
  keyCommit = setTimeout(() => commitAsOf(normalizeAsOf(next, dom)), 250);
}

function onResize() {
  scheduleTraces();
}

(function wire() {
  if (wired || typeof document === 'undefined') return;
  wired = true;
  document.addEventListener('click', onClick);
  document.addEventListener('pointerdown', onPointerDown);
  document.addEventListener('pointermove', onPointerMove);
  document.addEventListener('pointerup', onPointerUp);
  document.addEventListener('pointercancel', onPointerUp);
  document.addEventListener('keydown', onKeyDown);
  window.addEventListener('resize', onResize);
})();
