// ============================================================
// XERJ.ai — SECOND BRAIN · EGO LEDGER primitives
//
// The graph view is a LEDGER, not a hairball. A global force-directed
// graph was rejected on purpose: it collapses past ~200 nodes, its
// layout is non-deterministic (fatal for a memory you revisit — the
// same brain must always draw the same picture), and it optimises for
// looking at over navigating. The ledger is an ego view:
//
//   LEFT column   = inbound edges,  grouped by type
//   CENTRE        = the focus card (one node, its text, its provenance)
//   RIGHT column  = outbound edges, grouped by type
//
// Graph gestalt comes from 1px orthogonal traces drawn from the focus
// card to each GROUP HEADER (never to every row — zero occlusion, zero
// simulation). Hop 2 appears as a count on each neighbour row — a
// promise of the next query, not 27 more drawn nodes. The signature
// interaction is the BELIEF-TIME strip: every edge's validity interval
// on one time axis with a draggable as-of caret.
//
// Honesty rules (§0 invariant 8 of the second-brain contract): every
// cap this view applies is labelled in place ("TOP 12 OF 41"), the
// honesty panel always renders — zeros prove absence — and the
// lexical-embedder disclosure rides the timeline panel.
//
// Every export is a pure HTML-string renderer except
// `drawLedgerTraces`, which measures the live DOM (trace geometry
// depends on final text layout, so it must run post-paint).
// ============================================================

import { esc, fmt, num, Num } from './text.js';
import { Series, Dist } from './charts.js';

// Rows shown per edge-type group before the in-band "TOP n OF m" cap.
export const LEDGER_GROUP_CAP = 12;
// Validity rows drawn in the belief-time strip (top by weight, then
// valid_at, then edge_id — deterministic), cap labelled in the legend.
export const SCRUB_ROW_CAP = 64;

// ----- time ---------------------------------------------------------

/** Epoch-ms → 'YYYY-MM-DD HH:MM' (UTC — belief time is server time). */
export const fmtTs = (ms) => {
  if (ms == null || !Number.isFinite(Number(ms))) return '—';
  return new Date(Number(ms)).toISOString().slice(0, 16).replace('T', ' ');
};

/** Epoch-ms → 'YYYY-MM-DD' (UTC). Row-width-friendly. */
export const fmtDay = (ms) => {
  if (ms == null || !Number.isFinite(Number(ms))) return '—';
  return new Date(Number(ms)).toISOString().slice(0, 10);
};

/** An edge's validity window as text: '2026-07-25 → NOW' / '→ 2026-07-26'. */
const fmtWindow = (validAt, invalidAt) =>
  `${fmtDay(validAt)} → ${invalidAt == null ? 'NOW' : fmtDay(invalidAt)}`;

// ----- bi-temporal cut (client-side preview only) -------------------
//
// The SERVER owns the as-of cut (`ego?as_of=`). These helpers exist so
// the scrubber can preview visibility over already-fetched edges while
// the caret is mid-drag; the release commits an honest re-query.

export const edgeStateAt = (edge, asOfMs) => {
  const t = asOfMs == null ? Date.now() : asOfMs;
  if (Number(edge.valid_at) > t) return 'future';
  if (edge.invalid_at != null && Number(edge.invalid_at) <= t) return 'expired';
  return 'live';
};

// ----- neighbours + grouping ---------------------------------------

/** The node on the far end of an edge, given the expansion direction. */
const farEnd = (edge) => (edge.direction === 'in' ? edge.src : edge.dst);

/** Display name for a node id: hydrated title if present, else the id. */
const nodeName = (id, nodes) =>
  (nodes && nodes[id] && nodes[id].title) ? nodes[id].title : id;

/**
 * Hop-1 edges of one side, grouped by type. Order is deterministic:
 * groups by live-count desc then type asc; rows keep the server's
 * contract order (hop asc, weight desc, edge_id asc) untouched — the
 * same data always draws the same picture.
 */
export function groupEdges(edges, side) {
  const byType = new Map();
  for (const e of edges || []) {
    if (e.hop !== 1) continue;
    if ((side === 'out') !== (e.direction === 'out')) continue;
    if (!byType.has(e.type)) byType.set(e.type, []);
    byType.get(e.type).push(e);
  }
  const groups = [...byType.entries()].map(([type, list]) => ({ type, list }));
  groups.sort((a, b) => b.list.length - a.list.length || (a.type < b.type ? -1 : a.type > b.type ? 1 : 0));
  return groups;
}

/**
 * Per-neighbour hop-2 degree: how many hop-2 edges touch this node,
 * counting only edges live at the caret. When the server clipped the
 * result set the count is a floor, rendered '≥n' — never a fake exact.
 */
export function hop2Counts(edges, asOfMs) {
  const counts = new Map();
  for (const e of edges || []) {
    if (e.hop !== 2) continue;
    if (edgeStateAt(e, asOfMs) !== 'live') continue;
    counts.set(e.src, (counts.get(e.src) || 0) + 1);
    counts.set(e.dst, (counts.get(e.dst) || 0) + 1);
  }
  return counts;
}

// ----- evidence -----------------------------------------------------

/** The exact source quote that taught this edge, plus its full paper trail. */
const EvidenceBlock = (e) => {
  const ev = e.evidence || {};
  const quote = ev.quote
    ? `<p class="sb-evquote">“${esc(ev.quote)}”</p>`
    : `<p class="sb-evquote sb-evnone">no evidence recorded for this edge</p>`;
  const src = ev.source
    ? `<span>${esc(ev.source)}${ev.offset != null ? ` · byte ${esc(ev.offset)}` : ''}</span>`
    : '';
  const invalid = e.invalid_at != null
    ? ` · invalid ${esc(fmtTs(e.invalid_at))}` : '';
  const expired = e.expired_at != null
    ? ` · expiry recorded ${esc(fmtTs(e.expired_at))}` : '';
  return `
  <div class="sb-evbody" data-sb-evbody hidden>
    ${quote}
    <div class="sb-evmeta mono faint">
      ${src}
      <span>${esc(e.detector || 'unknown-detector')} · conf ${esc(fmt(e.confidence ?? 1, { decimals: 2 }))} · w ${esc(fmt(e.weight ?? 1, { decimals: 2 }))}</span>
      <span>valid ${esc(fmtTs(e.valid_at))}${invalid}${expired}</span>
      <span>edge ${esc(e.edge_id)}</span>
    </div>
  </div>`;
};

// ----- rows + groups ------------------------------------------------

const edgeRow = (e, side, { nodes, counts, asOf, clipped }) => {
  const id = farEnd(e);
  const state = edgeStateAt(e, asOf);
  const n = counts.get(id) || 0;
  const hop2 = n > 0
    ? (side === 'out'
      ? `· ${clipped ? '≥' : ''}${num(n)} →`
      : `← ${clipped ? '≥' : ''}${num(n)} ·`)
    : '';
  const evMark = (e.evidence && e.evidence.quote) ? 'ev' : '·';
  return `
  <div class="sb-erow${state === 'expired' ? ' sb-expired' : ''}${state === 'future' ? ' sb-off' : ''}"
       data-sb-edge="${esc(e.edge_id)}" data-va="${esc(e.valid_at)}"${e.invalid_at != null ? ` data-vi="${esc(e.invalid_at)}"` : ''}>
    <div class="sb-erow-main">
      <button type="button" class="sb-ename" data-sb-focus="${esc(id)}"
              title="Refocus the ledger on ${esc(id)}">${esc(nodeName(id, nodes))}</button>
      <button type="button" class="sb-emeta" data-sb-evidence="${esc(e.edge_id)}"
              title="Show the evidence that taught this edge">
        <span class="sb-ewin">${esc(fmtWindow(e.valid_at, e.invalid_at))}</span>
        <span class="sb-eev">${evMark}</span>
        <span class="sb-ehop2">${esc(hop2)}</span>
        <span class="sb-extag">EXPIRED</span>
      </button>
    </div>
    ${EvidenceBlock(e)}
  </div>`;
};

const groupBlock = (g, side, ctx) => {
  const total = g.list.length;
  const shown = Math.min(total, LEDGER_GROUP_CAP);
  const countLabel = shown < total
    ? `TOP ${num(shown)} OF ${num(total)}`
    : `${num(shown)} OF ${num(total)}`;
  const arrowType = side === 'in' ? `← ${g.type}` : `${g.type} →`;
  return `
  <div class="sb-group">
    <div class="sb-grouphead" data-sb-grouphead="${side}">
      <span class="sb-ghtype">${esc(arrowType.toUpperCase())}</span>
      <span class="sb-ghcount">${esc(countLabel)}</span>
    </div>
    ${g.list.slice(0, LEDGER_GROUP_CAP).map((e) => edgeRow(e, side, ctx)).join('')}
  </div>`;
};

// ----- focus card ---------------------------------------------------

const FocusCard = ({ ego, nodes, brain }) => {
  const id = ego.node;
  const hop1 = (ego.edges || []).filter((e) => e.hop === 1);
  const nIn = hop1.filter((e) => e.direction === 'in').length;
  const nOut = hop1.length - nIn;
  const types = new Set(hop1.map((e) => e.type)).size;
  const earliest = hop1.length
    ? Math.min(...hop1.map((e) => Number(e.valid_at)))
    : null;
  const meta = nodes && nodes[id];
  const preview = meta && meta.preview
    ? `<p class="sb-ftext">${esc(meta.preview)}</p>`
    : '';
  const dangling = !meta && ego.not_shown
    && Array.isArray(ego.not_shown.dangling_ids)
    && ego.not_shown.dangling_ids.includes(id);
  return `
  <div class="sb-focuscard" data-sb-focuscard>
    <div class="key">FOCUS</div>
    <div class="sb-ftitle">${esc(nodeName(id, nodes))}</div>
    ${preview}
    <div class="sb-fdeg mono">
      <span>← IN <span class="sb-fnum">${num(nIn)}</span></span>
      <span>OUT <span class="sb-fnum">${num(nOut)}</span> →</span>
      <span>${num(types)} TYPES</span>
    </div>
    <div class="sb-fprov mono faint">
      ${esc(meta && meta.index ? meta.index : `brain ${brain}`)} · ${esc(id)}${dangling ? ' · NO BACKING DOC' : ''}${earliest != null ? ` · in graph since ${esc(fmtDay(earliest))}` : ''}
    </div>
  </div>`;
};

// ----- controls line ------------------------------------------------

const EgoControls = ({ brain, brains, focus, asOf, trail }) => {
  const others = (brains || []).filter((b) => b !== brain);
  const brainSwitch = others.length
    ? `<span class="sb-cswitch">${others.map((b) =>
      `<button type="button" class="sb-clink" data-sb-brain="${esc(b)}">${esc(b)}</button>`).join('<span class="faint"> · </span>')}</span>`
    : '';
  const crumbs = (trail || []).filter((t) => t !== focus).slice(-4);
  const trailHtml = crumbs.length
    ? `<span class="sb-ctrail">${crumbs.map((t) =>
      `<button type="button" class="sb-clink" data-sb-focus="${esc(t)}">${esc(t)}</button>`).join('<span class="faint"> → </span>')}<span class="faint"> → </span></span>`
    : '';
  return `
  <div class="sb-controls mono">
    <span><span class="key">BRAIN</span> <span class="sb-cval">${esc(brain || '—')}</span> ${brainSwitch}</span>
    <span><span class="key">FOCUS</span> ${trailHtml}<span class="sb-cval">${esc(focus || '—')}</span></span>
    <span><span class="key">AS OF</span> <span class="sb-cval${asOf != null ? ' accent' : ''}">${asOf != null ? esc(fmtTs(asOf)) + ' UTC' : 'NOW'}</span>${asOf != null ? ` <button type="button" class="sb-clink" data-sb-now>RESET</button>` : ''}</span>
    <span class="sb-cerr" data-sb-error></span>
  </div>`;
};

// ----- belief-time scrubber ----------------------------------------

/**
 * Full-width strip of edge-validity intervals over one time axis with a
 * draggable as-of caret. The interval set comes from a timeline query
 * pinned at NOW with include_expired=true, so scrubbing left never
 * makes the strip itself lose rows — only their visibility state
 * changes. Dragging previews client-side; releasing re-queries the
 * server with `as_of` (the caret is a query parameter, not a filter).
 */
export const BeliefScrubber = ({ timeline, asOf }) => {
  const edges = (timeline && timeline.edges) || [];
  if (!edges.length) {
    return `<div class="sb-scrub" data-sb-scrub><div class="panel-empty mono faint">NO VALIDITY INTERVALS YET · EVERY EDGE DRAWS ITS LIFETIME HERE</div></div>`;
  }
  // Deterministic row order: weight desc, valid_at asc, edge_id asc.
  const rows = [...edges].sort((a, b) =>
    (b.weight - a.weight)
    || (Number(a.valid_at) - Number(b.valid_at))
    || (a.edge_id < b.edge_id ? -1 : 1)
  ).slice(0, SCRUB_ROW_CAP);

  const now = Date.now();
  const d0raw = Math.min(...rows.map((e) => Number(e.valid_at)));
  const span = Math.max(now - d0raw, 60_000);
  const d0 = d0raw - span * 0.04;         // 4% air before the first assertion
  const d1 = now;
  const pctOf = (ms) => Math.max(0, Math.min(100, ((ms - d0) / (d1 - d0)) * 100));

  const rowH = 4;
  const h = rows.length * rowH + 6;
  const caretMs = asOf == null ? d1 : asOf;
  const caretPct = pctOf(caretMs);

  const lines = rows.map((e, i) => {
    const y = 4 + i * rowH;
    const x1 = pctOf(Number(e.valid_at));
    const x2 = e.invalid_at != null ? pctOf(Number(e.invalid_at)) : 100;
    const state = edgeStateAt(e, asOf);
    const tick = e.invalid_at != null
      ? `<line class="sb-ltick" x1="${x2}%" x2="${x2}%" y1="${y - 2}" y2="${y + 2}" stroke="currentColor" stroke-width="1"/>`
      : '';
    return `<line class="sb-lseg sb-l-${state}" data-va="${esc(e.valid_at)}"${e.invalid_at != null ? ` data-vi="${esc(e.invalid_at)}"` : ''} x1="${x1}%" x2="${x2}%" y1="${y}" y2="${y}" stroke="currentColor" stroke-width="1"/>${tick}`;
  }).join('');

  const capNote = edges.length > rows.length
    ? ` · TOP ${num(rows.length)} OF ${num(edges.length)} BY WEIGHT`
    : '';

  return `
  <div class="sb-scrub" data-sb-scrub data-d0="${d0}" data-d1="${d1}">
    <div class="key">BELIEF TIME · DRAG THE CARET · WHAT DID THIS BRAIN BELIEVE, WHEN</div>
    <div class="sb-strip" data-sb-strip style="height:${h}px;">
      <svg class="chart sb-striplines" width="100%" height="${h}" aria-hidden="true">${lines}</svg>
      <div class="sb-caret" data-sb-caret style="left:${caretPct}%;" role="slider" tabindex="0"
           aria-label="Belief as-of caret" aria-valuemin="${d0}" aria-valuemax="${d1}" aria-valuenow="${caretMs}"></div>
      <div class="sb-caret-label mono" data-sb-caret-label style="left:${caretPct}%;">${asOf == null ? 'NOW' : esc(fmtTs(caretMs))}</div>
    </div>
    <div class="sb-scrub-legend mono faint">
      <span>${esc(fmtTs(d0raw))} UTC</span>
      <span class="mid">${num(rows.length)} VALIDITY INTERVALS${esc(capNote)} · EXPIRED EDGES KEEP THEIR ROW</span>
      <span><button type="button" class="text-btn" data-sb-now>NOW</button></span>
    </div>
  </div>`;
};

// ----- the ledger ---------------------------------------------------

const sideCol = (groups, side, ctx, label) => {
  const inner = groups.length
    ? groups.map((g) => groupBlock(g, side, ctx)).join('')
    : `<div class="sb-sidenote mono faint">${esc(label)}</div>`;
  return `<div class="sb-col sb-col-${side}">${inner}</div>`;
};

/** The three-column ego ledger + rails + scrubber. */
export const EgoLedger = ({ data }) => {
  const sb = data.sb || {};
  const ego = data.ego;
  const asOf = sb.asOf ?? null;
  if (!ego) {
    // Controls stay up even without a belief response so the user can
    // switch brain / reset the caret out of a dead end.
    return `
  ${EgoControls({ brain: data.brain, brains: sb.brains, focus: sb.focus, asOf, trail: sb.trail })}
  <div class="panel-empty mono faint">${sb.focus ? 'LOADING BELIEF · ' + esc(String(sb.focus).toUpperCase()) : 'NO FOCUS NODE · PICK A HUB BELOW TO OPEN THE LEDGER'}</div>`;
  }
  const nodes = ego.nodes || {};
  const clipped = !!(ego.not_shown && ego.not_shown.edges_clipped > 0);
  const counts = hop2Counts(ego.edges, asOf);
  const ctx = { nodes, counts, asOf, clipped };
  const inGroups = groupEdges(ego.edges, 'in');
  const outGroups = groupEdges(ego.edges, 'out');
  const hop1 = (ego.edges || []).filter((e) => e.hop === 1);

  const body = hop1.length
    ? `
  <div class="sb-ledger" data-sb-ledger>
    ${sideCol(inGroups, 'in', ctx, 'NOTHING POINTS HERE AT THIS AS-OF')}
    <div class="sb-rail" data-sb-rail="in" aria-hidden="true"></div>
    ${FocusCard({ ego, nodes, brain: data.brain })}
    <div class="sb-rail" data-sb-rail="out" aria-hidden="true"></div>
    ${sideCol(outGroups, 'out', ctx, 'THIS NODE POINTS AT NOTHING AT THIS AS-OF')}
  </div>`
    : `
  <div class="sb-ledger" data-sb-ledger>
    <div class="sb-col sb-col-in"></div>
    <div class="sb-rail" data-sb-rail="in" aria-hidden="true"></div>
    ${FocusCard({ ego, nodes, brain: data.brain })}
    <div class="sb-rail" data-sb-rail="out" aria-hidden="true"></div>
    <div class="sb-col sb-col-out"></div>
  </div>
  <div class="panel-empty mono faint">NO EDGES TOUCH ${esc(ego.node.toUpperCase())} AT THIS AS-OF · SCRUB TO NOW OR PICK A HUB BELOW</div>`;

  return `
  ${EgoControls({ brain: data.brain, brains: sb.brains, focus: ego.node, asOf, trail: sb.trail })}
  ${body}
  ${BeliefScrubber({ timeline: sb.timeline, asOf })}`;
};

// ----- honesty panel ------------------------------------------------

const hrow = (k, v, emph) => `
  <div class="sb-hrow">
    <span class="sb-hk">${esc(k)}</span>
    <span class="sb-hv${emph && v !== '0' ? ' accent' : ''}">${esc(v)}</span>
  </div>`;

/**
 * Definition list over every `not_shown` counter of both endpoints plus
 * the view's own caps. ALWAYS renders, zeros included — this panel
 * exists to prove absence, not just to report presence.
 */
export const HonestyList = ({ data }) => {
  const ns = (data.ego && data.ego.not_shown) || {};
  const on = (data.overview && data.overview.not_shown) || {};
  const z = (v) => num(v || 0);
  const dangling = Array.isArray(ns.dangling_ids) && ns.dangling_ids.length
    ? `<div class="sb-hids mono faint">dangling: ${ns.dangling_ids.slice(0, 8).map(esc).join(', ')}${ns.dangling_ids.length > 8 ? ` +${ns.dangling_ids.length - 8} more` : ''}</div>`
    : '';
  const timelineEdges = (data.sb && data.sb.timeline && data.sb.timeline.edges) || [];
  const scrubHidden = Math.max(0, timelineEdges.length - SCRUB_ROW_CAP);
  return `
  <div class="sb-honesty mono">
    <div class="sb-hgroup key">EGO QUERY</div>
    ${hrow('edges clipped', z(ns.edges_clipped), true)}
    ${hrow('frontier clipped', z(ns.frontier_clipped), true)}
    ${hrow('expired excluded', z(ns.expired_excluded), false)}
    ${hrow('type filtered', z(ns.type_filtered), false)}
    ${hrow('segments without columns', z(ns.segments_without_columns), true)}
    ${hrow('memtable docs scanned', z(ns.memtable_docs_scanned), false)}
    ${hrow('dangling node refs', z(ns.dangling_nodes), true)}
    ${dangling}
    <div class="sb-hgroup key">OVERVIEW AGGS</div>
    ${hrow('types not listed', z(on.types_not_listed), true)}
    ${hrow('detectors not listed', z(on.detectors_not_listed), true)}
    ${hrow('hubs (out) not listed', z(on.hubs_out_not_listed), false)}
    ${hrow('hubs (in) not listed', z(on.hubs_in_not_listed), false)}
    <div class="sb-hgroup key">THIS VIEW</div>
    ${hrow('ledger rows per group', `≤ ${num(LEDGER_GROUP_CAP)} (capped groups say so)`, false)}
    ${hrow('scrubber rows hidden', z(scrubHidden), false)}
  </div>`;
};

// ----- hubs ---------------------------------------------------------

const hubList = (items, notListed, side, focus) => {
  if (!items || !items.length) return `<div class="faint mono">No hubs yet.</div>`;
  const max = Math.max(...items.map((i) => i.live_edges), 1);
  const rows = items.map((i) => {
    const frac = Math.max(0, Math.min(1, i.live_edges / max));
    return `
    <div class="row clickable${i.id === focus ? ' sb-hub-active' : ''}" data-sb-focus="${esc(i.id)}" role="button"
         title="Refocus the ledger on ${esc(i.id)}">
      <div class="row__label">${esc(i.id)}</div>
      <div class="row__val">${esc(num(i.live_edges))}</div>
      <div class="row__bar">
        <svg class="chart" height="6" viewBox="0 0 200 6" preserveAspectRatio="none">
          <line x1="0" y1="5" x2="${(frac * 200).toFixed(1)}" y2="5" stroke="currentColor" stroke-width="1"/>
        </svg>
      </div>
      <div class="row__pct">${side === 'out' ? 'out' : 'in'}</div>
    </div>`;
  }).join('');
  const tail = notListed > 0
    ? `<div class="hint">+ ${num(notListed)} more not listed</div>` : '';
  return rows + tail;
};

export const HubsPanel = ({ data }) => {
  const o = data.overview || {};
  const hubs = o.hubs || { out: [], in: [] };
  const ns = o.not_shown || {};
  const focus = data.sb ? data.sb.focus : null;
  return `
  <div class="sb-hubs">
    <div>
      <div class="key">OUT · MOST ASSERTIVE</div>
      ${hubList(hubs.out, ns.hubs_out_not_listed, 'out', focus)}
    </div>
    <div>
      <div class="key">IN · MOST CITED</div>
      ${hubList(hubs.in, ns.hubs_in_not_listed, 'in', focus)}
    </div>
  </div>
  <div class="hint" style="margin-top:var(--sp-1);">click a hub to refocus the ledger</div>`;
};

// ----- small panels -------------------------------------------------

const TypeDistPanel = ({ data }) => {
  const o = data.overview || {};
  const types = o.types || [];
  if (!types.length) return `<div class="panel-empty mono faint">NO LIVE EDGES AT THIS AS-OF</div>`;
  const tail = o.not_shown && o.not_shown.types_not_listed > 0
    ? `<div class="hint">+ ${num(o.not_shown.types_not_listed)} types not listed</div>` : '';
  return Dist({ segments: types.map((t) => ({ label: t.type, value: t.live })) }) + tail;
};

const TimelinePanel = ({ data }) => {
  const o = data.overview || {};
  const series = (data.series && data.series.created) || [];
  const ts = (data.series && data.series.createdT) || [];
  // §0 invariant 8: the default embedder is lexical feature-hashing; any
  // surface that could imply neural semantics must say otherwise.
  const embedderNote = o.embedder === 'lexical-feature-hash'
    ? `<div class="hint" style="margin-top:var(--sp-1);">recall is lexical (feature hashing) — not neural</div>`
    : (o.embedder ? `<div class="hint" style="margin-top:var(--sp-1);">embedder · ${esc(o.embedder)}</div>` : '');
  if (series.length < 2) {
    const one = series.length === 1
      ? `<div class="mono">${esc(num(series[0]))} edges asserted · ${esc(fmtDay(ts[0]))}</div>` : '';
    return `${one || '<div class="panel-empty mono faint">NO ASSERTIONS RECORDED YET</div>'}${embedderNote}`;
  }
  return Series(series, {
    h: 120,
    labels: [fmtDay(ts[0]), fmtDay(ts[ts.length - 1])],
    unit: 'edges',
  }) + embedderNote;
};

const metricPanel = (data, id) => {
  const m = data.metrics && data.metrics[id];
  if (!m) return `<div class="panel-empty mono faint">—</div>`;
  return Num({
    value: m.formatted,
    unit: id === 'detectors' ? 'active' : 'edges',
    hint: m.hint,
    emphasis: id === 'edgesLive',
  });
};

// ----- empty / disconnected states ---------------------------------

/**
 * The dashboard reads the LIVE graph endpoints only — no demo numbers.
 * Anything else on this page would be a fake brain.
 */
const LiveOnlyNote = () => `
  <div class="sb-empty">
    <div class="mono">THIS DASHBOARD READS THE LIVE ENGINE · IT NEVER SHOWS DEMO DATA</div>
    <div class="mono faint" style="margin-top:var(--sp-1);">start xerj, point the console at it (SETTINGS → BACKEND · XERJ), reload</div>
  </div>`;

/** A brand-new brain: tell the user exactly how to fill it. */
export const EmptyBrainNote = ({ brain, connected, error }) => {
  if (!connected) {
    return `
  <div class="sb-empty">
    <div class="mono">ENGINE UNREACHABLE${error ? ` · ${esc(String(error).slice(0, 80).toUpperCase())}` : ''}</div>
    <div class="mono faint" style="margin-top:var(--sp-1);">start xerj and reload — this dashboard reads the live graph API only</div>
  </div>`;
  }
  const b = brain || 'notes';
  return `
  <div class="sb-empty">
    <div class="sb-ftitle">THIS BRAIN IS EMPTY</div>
    <p class="sb-ftext">No edges yet. A second brain grows from documents you already have —
    point the deterministic detectors at any folder and every wiki-link, relative link and
    file adjacency becomes an edge with evidence:</p>
    <pre class="sb-cmd mono">xerj autoindex ~/notes --brain ${esc(b)}</pre>
    <p class="sb-ftext">or assert the first edge yourself:</p>
    <pre class="sb-cmd mono">curl -X POST localhost:9200/_graph/${esc(b)}/link \\
  -H 'content-type: application/json' \\
  -d '{"src":"note-a","dst":"note-b","type":"cites"}'</pre>
    <div class="mono faint">then reload — the ledger, hubs and belief-time strip fill themselves</div>
  </div>`;
};

// ----- panel dispatcher ---------------------------------------------
//
// One function renders the INNER body of every second-brain panel so
// the interaction controller (data/second-brain-api.js) can re-render
// any panel in place after a refocus / as-of commit without the app's
// whole-page render loop.

export function renderPanelBody(id, data) {
  if (!data || !data.sb) return LiveOnlyNote();
  const sb = data.sb;
  const hasBrain = !!(data.overview && data.overview.exists !== false);
  const hasEdges = hasBrain && data.overview.edges && data.overview.edges.total > 0;

  switch (id) {
    case 'edgesLive':
    case 'edgesTotal':
    case 'invalidated':
    case 'detectors':
      return metricPanel(data, id);
    case 'typeDist':
      return hasEdges ? TypeDistPanel({ data }) : `<div class="panel-empty mono faint">NO EDGE TYPES YET</div>`;
    case 'edgeTimeline':
      return hasEdges ? TimelinePanel({ data }) : `<div class="panel-empty mono faint">NO ASSERTIONS YET</div>`;
    case 'ego':
      return hasEdges
        ? EgoLedger({ data })
        : EmptyBrainNote({ brain: data.brain, connected: sb.connected, error: sb.error });
    case 'hubs':
      return hasEdges ? HubsPanel({ data }) : `<div class="panel-empty mono faint">NO HUBS YET</div>`;
    case 'notShown':
      // Always renders — zeros prove absence.
      return HonestyList({ data });
    default:
      return `<div class="panel-empty mono faint">UNKNOWN PANEL · ${esc(id)}</div>`;
  }
}

// ----- post-paint trace drawing -------------------------------------

const crisp = (v) => Math.round(v) + 0.5;

/**
 * Draw the 1px orthogonal traces from the focus card to each group
 * header. Runs against the live DOM because the endpoints are text
 * blocks whose heights depend on final layout — this is a measure +
 * draw pass, not a simulation. Same data + same viewport ⇒ same
 * traces (deterministic by construction).
 */
export function drawLedgerTraces(scope) {
  if (typeof document === 'undefined') return;
  const root = scope || document;
  const ledger = root.querySelector('[data-sb-ledger]');
  if (!ledger) return;
  const card = ledger.querySelector('[data-sb-focuscard]');
  if (!card) return;
  const cardRect = card.getBoundingClientRect();
  for (const side of ['in', 'out']) {
    const rail = ledger.querySelector(`[data-sb-rail="${side}"]`);
    if (!rail) continue;
    const rr = rail.getBoundingClientRect();
    if (rr.width < 8 || rr.height < 8) { rail.innerHTML = ''; continue; }
    const W = rr.width;
    const H = rr.height;
    const fy = crisp(Math.max(6, Math.min(H - 6, cardRect.top + cardRect.height / 2 - rr.top)));
    const midX = crisp(W / 2);
    let d = '';
    for (const head of ledger.querySelectorAll(`[data-sb-grouphead="${side}"]`)) {
      const hr = head.getBoundingClientRect();
      const hy = crisp(Math.max(2, Math.min(H - 2, hr.top + hr.height / 2 - rr.top)));
      // Left rail: header edge at x=0, focus edge at x=W. Right rail mirrored.
      d += side === 'in'
        ? `M0 ${hy} H${midX} V${fy} H${W} `
        : `M${W} ${hy} H${midX} V${fy} H0 `;
    }
    rail.innerHTML = d
      ? `<svg class="sb-trace" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" aria-hidden="true"><path d="${d}" fill="none" stroke="currentColor" stroke-width="1"/></svg>`
      : '';
  }
}
