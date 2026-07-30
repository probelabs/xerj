// ============================================================
// XERJ.ai — SECOND BRAIN · THE MAP — pipeline (pure, deterministic)
//
// Turns a weight-ranked bulk fetch of a brain's links into a bounded
// picture: intern → contract reading-order chains → connected
// components → pinned-order weighted label propagation → rank. The
// output is ≤ MAP_TOP_CLUSTERS named groups + one "everything else"
// body — the node count on screen is bounded BY CONSTRUCTION, never by
// corpus luck.
//
// Determinism is a product requirement (the same brain must always
// draw the same picture):
//   - every stage's processing order is pinned (degree desc, interned
//     id asc — never hash-map iteration order);
//   - the only randomness is init jitter from mulberry32 seeded by the
//     brain's name;
//   - layout is a fixed 300-iteration relaxation run synchronously at
//     build time — a pure function of the link set, frozen after.
//     There is NO live simulation anywhere.
//
// This module is pure (no DOM, no fetch) so it runs headless under
// node for verification. The canvas view lives in brain-map.js.
// ============================================================

// ----- normative constants (the design brief, §2.1) -----------------

export const MAP_PAGE = 5_000; // per _search page
export const MAP_EDGE_BUDGET = 50_000; // hard cap — 10 pages max
export const MAP_TOP_CLUSTERS = 12; // + 1 "everything else" = ≤ 13
export const MAP_SMALL_DIRECT = 40; // ≤ this many file-nodes → no clustering
export const MAP_EXPAND_CAP = 150; // members drawn on expand
export const MAP_PERCOLATION_MAX = 0.35; // cluster > 35% of component → re-split
export const MAP_LPA_SWEEPS = 10;
export const MAP_LAYOUT_ITERS = 300;

// ----- seeded rng ---------------------------------------------------

/** FNV-1a over a string → uint32 seed. */
export function hashStr(s) {
  let h = 0x811c9dc5;
  const str = String(s || '');
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** mulberry32 — tiny deterministic PRNG. */
export function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0; a = (a + 0x6D2B79F5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ----- stage 0: intern ----------------------------------------------

/**
 * Raw fetched links → dense typed arrays. Input order (weight desc,
 * link id asc — the server sort) is the interning order, so the same
 * fetch always produces the same dense ids.
 *
 * rows: [{ id, src, dst, type, weight, validAt, invalidAt|null, srcFile }]
 */
export function internEdges(rows) {
  const idOf = new Map();
  const ids = [];
  const intern = (s) => {
    let i = idOf.get(s);
    if (i === undefined) { i = ids.length; ids.push(s); idOf.set(s, i); }
    return i;
  };
  const n = rows.length;
  const srcI = new Uint32Array(n);
  const dstI = new Uint32Array(n);
  const weight = new Float32Array(n);
  const typeOrd = new Uint8Array(n);
  const validAt = new Float64Array(n);
  const invalidAt = new Float64Array(n); // NaN = still believed
  const edgeIds = new Array(n);
  const srcFile = new Array(n);
  const typeNames = [];
  const typeIdx = new Map();
  for (let i = 0; i < n; i++) {
    const r = rows[i];
    srcI[i] = intern(r.src);
    dstI[i] = intern(r.dst);
    weight[i] = Number.isFinite(r.weight) ? r.weight : 1;
    let t = typeIdx.get(r.type);
    if (t === undefined) { t = typeNames.length; typeNames.push(String(r.type || 'link')); typeIdx.set(r.type, t); }
    typeOrd[i] = t;
    validAt[i] = Number(r.validAt) || 0;
    invalidAt[i] = r.invalidAt == null ? NaN : Number(r.invalidAt);
    edgeIds[i] = r.id;
    srcFile[i] = r.srcFile || '';
  }
  return { n, ids, srcI, dstI, weight, typeOrd, typeNames, validAt, invalidAt, edgeIds, srcFile };
}

/** 'live' | 'expired' | 'future' for interned edge i at as-of t. */
export function edgeStateAtI(g, i, t) {
  const at = t == null ? Date.now() : t;
  if (g.validAt[i] > at) return 'future';
  const inv = g.invalidAt[i];
  if (!Number.isNaN(inv) && inv <= at) return 'expired';
  return 'live';
}

// ----- union-find (union-by-min-root — deterministic) ---------------

function ufMake(n) { const p = new Int32Array(n); for (let i = 0; i < n; i++) p[i] = i; return p; }
function ufFind(p, x) {
  let r = x;
  while (p[r] !== r) r = p[r];
  while (p[x] !== r) { const nx = p[x]; p[x] = r; x = nx; }
  return r;
}
function ufUnion(p, a, b) {
  const ra = ufFind(p, a); const rb = ufFind(p, b);
  if (ra === rb) return;
  if (ra < rb) p[rb] = ra; else p[ra] = rb;
}

// ----- stage 1: contract reading-order chains -----------------------

/**
 * Sections of one document are one document: union over `sequence`
 * links only, then remap every link onto file-nodes. Parallel links
 * between the same file pair merge (summed weight, per-kind counts,
 * constituent list kept for the bundle inspector). Links inside one
 * document leave the picture but are counted.
 */
export function contractSequence(g) {
  const seqOrd = g.typeNames.indexOf('sequence');
  const p = ufMake(g.ids.length);
  if (seqOrd >= 0) {
    for (let i = 0; i < g.n; i++) {
      if (g.typeOrd[i] === seqOrd) ufUnion(p, g.srcI[i], g.dstI[i]);
    }
  }
  // Dense file ids in root order (root = min member ⇒ deterministic).
  const fileOfRoot = new Map();
  const fileOf = new Int32Array(g.ids.length);
  const fileRep = []; // representative ORIGINAL interned id per file-node
  const fileSize = []; // notes (original nodes) per file-node
  for (let v = 0; v < g.ids.length; v++) {
    const r = ufFind(p, v);
    let f = fileOfRoot.get(r);
    if (f === undefined) { f = fileRep.length; fileOfRoot.set(r, f); fileRep.push(r); fileSize.push(0); }
    fileOf[v] = f;
    fileSize[f] += 1;
  }
  const F = fileRep.length;
  // Merge parallel links between file pairs (undirected).
  const pairIdx = new Map(); // key a*F+b (a<b) → merged idx
  const fa = []; const fb = []; const fWeight = []; const fCount = [];
  const fKind = []; // per merged edge: Map(typeOrd → count) — insertion order deterministic
  const fEdges = []; // constituent original edge indices
  let selfLoops = 0;
  for (let i = 0; i < g.n; i++) {
    if (seqOrd >= 0 && g.typeOrd[i] === seqOrd) {
      // Contracted away — but COUNTED: every reading-order link lives
      // inside one document by construction, and the "links inside
      // single documents" honesty row must reconcile with the fetch
      // total (fetched = Σ merged-link constituents + selfLoops).
      selfLoops += 1;
      continue;
    }
    let a = fileOf[g.srcI[i]]; let b = fileOf[g.dstI[i]];
    if (a === b) { selfLoops += 1; continue; }
    if (a > b) { const t = a; a = b; b = t; }
    const key = a * F + b;
    let m = pairIdx.get(key);
    if (m === undefined) {
      m = fa.length;
      pairIdx.set(key, m);
      fa.push(a); fb.push(b); fWeight.push(0); fCount.push(0);
      fKind.push(new Map()); fEdges.push([]);
    }
    fWeight[m] += g.weight[i];
    fCount[m] += 1;
    fKind[m].set(g.typeOrd[i], (fKind[m].get(g.typeOrd[i]) || 0) + 1);
    fEdges[m].push(i);
  }
  return { F, fileOf, fileRep, fileSize, fa, fb, fWeight, fCount, fKind, fEdges, selfLoops, seqOrd };
}

// ----- stage 2: connected components --------------------------------

export function components(fg) {
  const p = ufMake(fg.F);
  for (let m = 0; m < fg.fa.length; m++) ufUnion(p, fg.fa[m], fg.fb[m]);
  const compOf = new Int32Array(fg.F);
  const sizeOfRoot = new Map();
  for (let f = 0; f < fg.F; f++) {
    const r = ufFind(p, f);
    compOf[f] = r;
    sizeOfRoot.set(r, (sizeOfRoot.get(r) || 0) + 1);
  }
  return { compOf, sizeOfRoot };
}

// ----- CSR adjacency over merged file edges -------------------------

function buildCsr(fg, edgeKeep) {
  const F = fg.F;
  const deg = new Uint32Array(F);
  const M = fg.fa.length;
  for (let m = 0; m < M; m++) {
    if (edgeKeep && !edgeKeep(m)) continue;
    deg[fg.fa[m]] += 1; deg[fg.fb[m]] += 1;
  }
  const off = new Uint32Array(F + 1);
  for (let f = 0; f < F; f++) off[f + 1] = off[f] + deg[f];
  const nbr = new Uint32Array(off[F]);
  const nbrW = new Float64Array(off[F]);
  const cursor = off.slice(0, F);
  for (let m = 0; m < M; m++) {
    if (edgeKeep && !edgeKeep(m)) continue;
    const a = fg.fa[m]; const b = fg.fb[m]; const w = fg.fWeight[m];
    nbr[cursor[a]] = b; nbrW[cursor[a]] = w; cursor[a] += 1;
    nbr[cursor[b]] = a; nbrW[cursor[b]] = w; cursor[b] += 1;
  }
  return { off, nbr, nbrW, deg };
}

// ----- stage 3: pinned-order weighted label propagation -------------

/**
 * Per component of ≥ 3 file-nodes. Labels init to self; nodes
 * processed in (degree desc, id asc) order; each node adopts the
 * neighbour label with the largest incident weight, ties → smallest
 * label; ≤ MAP_LPA_SWEEPS sweeps or < 0.5% churn. The detector weight
 * ladder (authored links weigh 2–3× folder adjacency) makes citations
 * dominate community formation without any semantic claim.
 *
 * Percolation guard: a cluster > MAP_PERCOLATION_MAX of its component
 * re-runs once with folder-neighbour-only links excluded; nodes losing
 * every incident link keep their parent label; if the giant survives
 * that, it is accepted — some corpora genuinely have one big topic and
 * faking a split would be a lie.
 */
export function labelProp(fg, comps) {
  const F = fg.F;
  const run = (edgeKeep, initLabels) => {
    const csr = buildCsr(fg, edgeKeep);
    const labels = initLabels ? initLabels.slice() : (() => { const l = new Int32Array(F); for (let f = 0; f < F; f++) l[f] = f; return l; })();
    const order = [];
    for (let f = 0; f < F; f++) order.push(f);
    order.sort((a, b) => (csr.deg[b] - csr.deg[a]) || (a - b));
    const tally = new Float64Array(F);
    const touched = [];
    for (let sweep = 0; sweep < MAP_LPA_SWEEPS; sweep++) {
      let churn = 0;
      for (const f of order) {
        const s = csr.off[f]; const e = csr.off[f + 1];
        if (s === e) continue; // no incident links → keeps its label
        for (let k = s; k < e; k++) {
          const lab = labels[csr.nbr[k]];
          if (tally[lab] === 0) touched.push(lab);
          tally[lab] += csr.nbrW[k];
        }
        let best = labels[f]; let bestW = tally[best] || 0;
        for (const lab of touched) {
          const w = tally[lab];
          if (w > bestW || (w === bestW && lab < best)) { best = lab; bestW = w; }
        }
        for (const lab of touched) tally[lab] = 0;
        touched.length = 0;
        if (best !== labels[f]) { labels[f] = best; churn += 1; }
      }
      if (churn / Math.max(1, F) < 0.005) break;
    }
    return labels;
  };

  let labels = run(null, null);

  // Percolation guard per component.
  const compSize = comps.sizeOfRoot;
  const clusterSize = new Map();
  for (let f = 0; f < F; f++) {
    const key = `${comps.compOf[f]}:${labels[f]}`;
    clusterSize.set(key, (clusterSize.get(key) || 0) + 1);
  }
  let needResplit = false;
  for (const [key, size] of clusterSize) {
    const comp = Number(key.split(':')[0]);
    const cs = compSize.get(comp) || 1;
    if (cs >= 3 && size / cs > MAP_PERCOLATION_MAX && size > 3) { needResplit = true; break; }
  }
  if (needResplit) {
    // Exclude merged links that are ONLY folder adjacency (same_dir).
    // A merged link keeping any authored/sequence-adjacent weight stays.
    const sameDirOnly = (m) => {
      const kinds = fg.fKind[m];
      if (kinds.size !== 1) return false;
      const [ord] = kinds.keys();
      return fg.typeNamesRef ? fg.typeNamesRef[ord] === 'same_dir' : false;
    };
    labels = run((m) => !sameDirOnly(m), labels);
  }
  return labels;
}

// ----- stage 4+5: rank + name inputs --------------------------------

/**
 * Clusters ranked by Σ weight of internal links live at `asOf`. Top
 * MAP_TOP_CLUSTERS become bodies; the rest — plus components < 3
 * file-nodes — pool into "everything else". Naming stays honest: a
 * cluster is labelled by its dominant folder prefix when ≥ 60% of its
 * links come from one, else by its busiest member's title (hydrated
 * by the caller — this stage only nominates the id).
 */
export function rankClusters(g, fg, comps, labels, asOf) {
  const F = fg.F;
  // Group file-nodes: key = component root + label; pool small comps.
  const groups = new Map(); // key → { files: [] }
  for (let f = 0; f < F; f++) {
    const comp = comps.compOf[f];
    const cs = comps.sizeOfRoot.get(comp) || 1;
    const key = cs < 3 ? '__else' : `${comp}:${labels[f]}`;
    let grp = groups.get(key);
    if (!grp) { grp = { key, files: [] }; groups.set(key, grp); }
    grp.files.push(f);
  }
  // Cluster of each file for internal/external classification (prelim).
  const groupOf = new Int32Array(F).fill(-1);
  const groupList = [...groups.values()];
  groupList.forEach((grp, gi) => { for (const f of grp.files) groupOf[f] = gi; });

  // Internal live-weight score per group + link counts, over ORIGINAL
  // links (excluding contracted reading-order ones).
  const score = new Float64Array(groupList.length);
  const linkTotal = new Uint32Array(groupList.length);
  const linkLive = new Uint32Array(groupList.length);
  for (let m = 0; m < fg.fa.length; m++) {
    const ga = groupOf[fg.fa[m]]; const gb = groupOf[fg.fb[m]];
    if (ga !== gb) continue;
    for (const i of fg.fEdges[m]) {
      linkTotal[ga] += 1;
      if (edgeStateAtI(g, i, asOf) === 'live') { linkLive[ga] += 1; score[ga] += g.weight[i]; }
    }
  }

  // Rank: score desc, then smallest member file id asc (stable).
  const order = groupList.map((grp, gi) => gi)
    .filter((gi) => groupList[gi].key !== '__else')
    .sort((a, b) => (score[b] - score[a]) || (Math.min(...groupList[a].files) - Math.min(...groupList[b].files)));

  const top = order.slice(0, MAP_TOP_CLUSTERS);
  const pooled = order.slice(MAP_TOP_CLUSTERS);
  const elseGi = groupList.findIndex((grp) => grp.key === '__else');

  // Build final clusters: 0..k-1 real, k = "everything else" (if any).
  const clusters = [];
  const clusterOfFile = new Int32Array(F).fill(-1);
  for (const gi of top) {
    const files = groupList[gi].files.slice().sort((a, b) => a - b);
    const ci = clusters.length;
    for (const f of files) clusterOfFile[f] = ci;
    clusters.push({
      files, isElse: false,
      score: score[gi], linkTotal: linkTotal[gi], linkLive: linkLive[gi],
      noteCount: files.reduce((acc, f) => acc + fg.fileSize[f], 0),
    });
  }
  const elseFiles = [];
  for (const gi of pooled) elseFiles.push(...groupList[gi].files);
  if (elseGi >= 0) elseFiles.push(...groupList[elseGi].files);
  if (elseFiles.length) {
    elseFiles.sort((a, b) => a - b);
    const ci = clusters.length;
    for (const f of elseFiles) clusterOfFile[f] = ci;
    // Count internal links of the pooled body (recompute over membership).
    let lt = 0; let ll = 0; let sc = 0;
    for (let m = 0; m < fg.fa.length; m++) {
      if (clusterOfFile[fg.fa[m]] !== ci || clusterOfFile[fg.fb[m]] !== ci) continue;
      for (const i of fg.fEdges[m]) {
        lt += 1;
        if (edgeStateAtI(g, i, asOf) === 'live') { ll += 1; sc += g.weight[i]; }
      }
    }
    clusters.push({
      files: elseFiles, isElse: true,
      score: sc, linkTotal: lt, linkLive: ll,
      noteCount: elseFiles.reduce((acc, f) => acc + fg.fileSize[f], 0),
    });
  }

  // Naming inputs per cluster.
  for (let ci = 0; ci < clusters.length; ci++) {
    const c = clusters[ci];
    // Dominant folder over internal links' source files: the DEEPEST
    // directory prefix that still covers ≥ 60% of them. Depth matters —
    // on a corpus that lives entirely under one root, the first path
    // segment names every cluster identically and says nothing.
    const prefixCount = new Map(); // 'a/b' → count; '' = top level
    let total = 0;
    for (let m = 0; m < fg.fa.length; m++) {
      const ca = clusterOfFile[fg.fa[m]]; const cb = clusterOfFile[fg.fb[m]];
      if (ca !== ci || cb !== ci) continue;
      for (const i of fg.fEdges[m]) {
        const sf = g.srcFile[i];
        const segs = sf.split('/');
        segs.pop(); // filename off
        for (let d = 0; d <= segs.length; d++) {
          const p = segs.slice(0, d).join('/');
          prefixCount.set(p, (prefixCount.get(p) || 0) + 1);
        }
        total += 1;
      }
    }
    let bestDir = null; let bestDepth = -1;
    for (const [p, cnt] of prefixCount) {
      if (total === 0 || cnt / total < 0.6) continue;
      const depth = p === '' ? 0 : p.split('/').length;
      if (depth > bestDepth || (depth === bestDepth && bestDir != null && p < bestDir)) {
        bestDir = p; bestDepth = depth;
      }
    }
    c.dirLabel = bestDir == null ? null : (bestDir === '' ? '(top level)' : `${bestDir}/`);
    // Busiest member (live merged degree desc, id asc) → hub nominee.
    const degLive = new Map();
    for (let m = 0; m < fg.fa.length; m++) {
      let live = 0;
      for (const i of fg.fEdges[m]) if (edgeStateAtI(g, i, asOf) === 'live') live += 1;
      if (!live) continue;
      degLive.set(fg.fa[m], (degLive.get(fg.fa[m]) || 0) + live);
      degLive.set(fg.fb[m], (degLive.get(fg.fb[m]) || 0) + live);
    }
    c.hubFile = c.files.slice().sort((a, b) => ((degLive.get(b) || 0) - (degLive.get(a) || 0)) || (a - b))[0];
    c.degLive = degLive; // reused for member ordering on expand
  }

  // Two clusters can honestly share a folder name (both ≥ 60% inside
  // engine/, no deeper prefix dominant) — mark the duplicates so the
  // view can add each one's busiest note and the reader can tell them
  // apart (live-seen on the repo brain: engine/ ×2, (top level) ×2).
  const labelCount = new Map();
  for (const c of clusters) if (c.dirLabel) labelCount.set(c.dirLabel, (labelCount.get(c.dirLabel) || 0) + 1);
  for (const c of clusters) c.dirDupe = !!(c.dirLabel && labelCount.get(c.dirLabel) > 1);

  return { clusters, clusterOfFile };
}

// ----- bundles between bodies ---------------------------------------

/** Aggregate merged file links crossing cluster boundaries into ≤
 *  C·(C−1)/2 bundles (78 worst case at 13 bodies). */
export function buildBundles(fg, clusterOfFile, nClusters) {
  const idx = new Map();
  const bundles = [];
  for (let m = 0; m < fg.fa.length; m++) {
    let a = clusterOfFile[fg.fa[m]]; let b = clusterOfFile[fg.fb[m]];
    if (a < 0 || b < 0 || a === b) continue;
    if (a > b) { const t = a; a = b; b = t; }
    const key = a * nClusters + b;
    let bi = idx.get(key);
    if (bi === undefined) {
      bi = bundles.length;
      idx.set(key, bi);
      bundles.push({ a, b, count: 0, weight: 0, merged: [] });
    }
    const bu = bundles[bi];
    bu.count += fg.fCount[m];
    bu.weight += fg.fWeight[m];
    bu.merged.push(m);
  }
  return bundles;
}

/** Live constituent count of a bundle at as-of. */
export function bundleLiveAt(g, fg, bundle, asOf) {
  let live = 0;
  for (const m of bundle.merged) {
    for (const i of fg.fEdges[m]) if (edgeStateAtI(g, i, asOf) === 'live') live += 1;
  }
  return live;
}

// ----- layout: seeded relaxation (NOT a live simulation) ------------

/**
 * Place `nb` bodies in the unit square: init on a circle in rank
 * order + seeded jitter, then exactly MAP_LAYOUT_ITERS iterations of
 * Fruchterman–Reingold arithmetic, run synchronously, output frozen.
 * A pure function of (edges, seed): same brain → same picture.
 */
export function relaxLayout(nb, links, seedStr) {
  const rng = mulberry32(hashStr(seedStr));
  const px = new Float64Array(nb);
  const py = new Float64Array(nb);
  const R = 0.35;
  for (let i = 0; i < nb; i++) {
    const th = (i / Math.max(1, nb)) * 2 * Math.PI - Math.PI / 2;
    px[i] = 0.5 + R * Math.cos(th) + (rng() - 0.5) * 0.02;
    py[i] = 0.5 + R * Math.sin(th) + (rng() - 0.5) * 0.02;
  }
  if (nb <= 1) return { px, py };
  const k = Math.sqrt(1 / nb) * 0.9;
  const maxW = links.reduce((a, l) => Math.max(a, l.w), 0) || 1;
  const dx = new Float64Array(nb);
  const dy = new Float64Array(nb);
  for (let iter = 0; iter < MAP_LAYOUT_ITERS; iter++) {
    dx.fill(0); dy.fill(0);
    for (let i = 0; i < nb; i++) {
      for (let j = i + 1; j < nb; j++) {
        let ddx = px[i] - px[j]; let ddy = py[i] - py[j];
        let d2 = ddx * ddx + ddy * ddy;
        if (d2 < 1e-8) { ddx = 1e-4 * ((i + 1) % 3 - 1); ddy = 1e-4; d2 = ddx * ddx + ddy * ddy; }
        const rep = (k * k) / d2;
        dx[i] += ddx * rep; dy[i] += ddy * rep;
        dx[j] -= ddx * rep; dy[j] -= ddy * rep;
      }
    }
    for (const l of links) {
      const ddx = px[l.a] - px[l.b]; const ddy = py[l.a] - py[l.b];
      const d = Math.sqrt(ddx * ddx + ddy * ddy) || 1e-6;
      const att = (d * d / k) * (0.3 + 0.7 * (l.w / maxW));
      const ux = (ddx / d) * att; const uy = (ddy / d) * att;
      dx[l.a] -= ux; dy[l.a] -= uy;
      dx[l.b] += ux; dy[l.b] += uy;
    }
    // Center gravity: sparsely-bundled bodies must drift inward, not
    // pin to the walls — without it pure repulsion shoves every
    // disconnected body into the clamp corners. Factor swept on the
    // repo's own docs/ brain (13 bodies, 5 bundles): 10k → 0 of 13
    // wall-pinned, min pair distance 0.21 of the unit square.
    // (Gravity swept again after the post-layout spread rescale landed:
    // 10k crushed 13 cross-linked bodies into a third of the frame; 4k
    // keeps disconnected bodies off the walls — the rescale below now
    // guarantees frame fill — while letting linked bodies breathe.)
    for (let i = 0; i < nb; i++) {
      dx[i] -= (px[i] - 0.5) * k * 4;
      dy[i] -= (py[i] - 0.5) * k * 4;
    }
    const temp = 0.1 * (1 - iter / MAP_LAYOUT_ITERS) + 0.002;
    for (let i = 0; i < nb; i++) {
      const d = Math.sqrt(dx[i] * dx[i] + dy[i] * dy[i]) || 1e-9;
      const step = Math.min(d, temp);
      px[i] += (dx[i] / d) * step;
      py[i] += (dy[i] / d) * step;
      px[i] = Math.max(0.03, Math.min(0.97, px[i]));
      py[i] = Math.max(0.03, Math.min(0.97, py[i]));
    }
  }
  // Fill the frame: the center gravity that keeps disconnected bodies
  // off the walls also compresses the finished picture into the middle
  // of the unit square (live-seen on the 19k-link repo brain — 13
  // bodies crowded into ~a third of the canvas). Affine per-axis
  // spread, capped at 3× so a genuinely tight pair is not flung to the
  // corners. Pure arithmetic on the frozen positions — determinism
  // holds: same positions in, same positions out.
  let xmin = Infinity; let xmax = -Infinity; let ymin = Infinity; let ymax = -Infinity;
  for (let i = 0; i < nb; i++) {
    xmin = Math.min(xmin, px[i]); xmax = Math.max(xmax, px[i]);
    ymin = Math.min(ymin, py[i]); ymax = Math.max(ymax, py[i]);
  }
  const target = 0.88; // world span between the 0.06 margins
  const fx = xmax - xmin > 1e-6 ? Math.min(target / (xmax - xmin), 3) : 0;
  const fy = ymax - ymin > 1e-6 ? Math.min(target / (ymax - ymin), 3) : 0;
  const cx0 = (xmin + xmax) / 2; const cy0 = (ymin + ymax) / 2;
  for (let i = 0; i < nb; i++) {
    px[i] = 0.5 + (fx ? (px[i] - cx0) * fx : 0);
    py[i] = 0.5 + (fy ? (py[i] - cy0) * fy : 0);
  }
  return { px, py };
}

/** Golden-angle spiral: member slot i around a frozen centroid.
 *  O(n), zero iterations, zero occlusion, deterministic. */
export function phyllotaxis(i, cx, cy, c) {
  const th = i * 137.507 * (Math.PI / 180);
  const r = c * Math.sqrt(i);
  return [cx + r * Math.cos(th), cy + r * Math.sin(th)];
}

// ----- the whole pipeline -------------------------------------------

/**
 * rows (fetched, weight-desc) + brainName + asOf → the frozen model.
 * mode 'direct' (≤ MAP_SMALL_DIRECT file-nodes: the real graph, named
 * per file) or 'clustered' (≤ 13 bodies).
 */
export function buildMap(rows, { brainName, asOf = null } = {}) {
  const g = internEdges(rows);
  const fg = contractSequence(g);
  fg.typeNamesRef = g.typeNames; // for the percolation guard's same_dir test
  const distinctNotes = g.ids.length;

  if (fg.F <= MAP_SMALL_DIRECT) {
    // Small corpus: no clustering theatre — the real file graph.
    const links = fg.fa.map((a, m) => ({ a, b: fg.fb[m], w: fg.fWeight[m] }));
    const { px, py } = relaxLayout(fg.F, links, brainName);
    return { mode: 'direct', g, fg, distinctNotes, px, py };
  }

  const comps = components(fg);
  const labels = labelProp(fg, comps);
  const { clusters, clusterOfFile } = rankClusters(g, fg, comps, labels, asOf);
  const bundles = buildBundles(fg, clusterOfFile, clusters.length);
  const links = bundles.map((bu) => ({ a: bu.a, b: bu.b, w: bu.weight }));
  const { px, py } = relaxLayout(clusters.length, links, brainName);
  return { mode: 'clustered', g, fg, distinctNotes, clusters, clusterOfFile, bundles, px, py };
}

/** Recompute the as-of-dependent numbers (scores, live counts) after a
 *  belief-time commit. Membership and layout NEVER change here. */
export function restateAt(model, asOf) {
  if (model.mode !== 'clustered') return;
  const { g, fg, clusters, clusterOfFile } = model;
  for (const c of clusters) { c.linkLive = 0; c.score = 0; }
  for (let m = 0; m < fg.fa.length; m++) {
    const ca = clusterOfFile[fg.fa[m]]; const cb = clusterOfFile[fg.fb[m]];
    if (ca < 0 || ca !== cb) continue;
    for (const i of fg.fEdges[m]) {
      if (edgeStateAtI(g, i, asOf) === 'live') {
        clusters[ca].linkLive += 1;
        clusters[ca].score += g.weight[i];
      }
    }
  }
}

/** Members of a cluster in draw order (live merged degree desc, file
 *  id asc), capped by MAP_EXPAND_CAP by the caller. */
export function memberOrder(cluster) {
  const deg = cluster.degLive || new Map();
  return cluster.files.slice().sort((a, b) => ((deg.get(b) || 0) - (deg.get(a) || 0)) || (a - b));
}
