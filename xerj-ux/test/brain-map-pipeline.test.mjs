// THE MAP — offline contract tests for the bounded knowledge-graph pipeline.
//
// The pipeline is pure, deterministic and dependency-free, so the claims it
// is sold on are testable without a browser, a server or a corpus:
//
//   * "13 groups at any scale"  — clusters.length ≤ MAP_TOP_CLUSTERS + 1
//   * "byte-identical across runs" — same rows in, same partition out
//   * the honesty row must reconcile: every fetched link is either a merged
//     link constituent or a counted self-loop, never silently dropped
//
// Run: node --test xerj-ux/test/
import test from 'node:test';
import assert from 'node:assert/strict';

import {
  MAP_TOP_CLUSTERS,
  mulberry32,
  internEdges,
  contractSequence,
  components,
  labelProp,
  rankClusters,
  buildBundles,
} from '../src/ux/brain-map-pipeline.js';

const DAY = 86_400_000;
const T0 = 1_700_000_000_000; // fixed epoch — no Date.now() in fixtures

/**
 * A synthetic vault with real community structure: `folders` folders,
 * `filesPerFolder` documents each, every document split into `sections`
 * sections chained by `sequence` links (so stage 1 has something to
 * contract). Cross-file links are dense inside a folder and sparse
 * across folders, which is what makes label propagation find groups
 * rather than one blob.
 *
 * A fraction of links are retired (invalidAt in the past) so the
 * live/total accounting is exercised too.
 */
function makeVault({ folders, filesPerFolder, sections = 3, crossPerFile = 4, seed = 42 }) {
  const rnd = mulberry32(seed);
  const rows = [];
  const node = (fo, fi, s) => `f${fo}/doc${fi}#s${s}`;
  const path = (fo, fi) => `vault/area${fo}/doc${fi}.md`;
  let id = 0;
  const push = (src, dst, type, srcFile, weight) => {
    // ~8% of links are retired before the as-of instant we query at.
    const retired = rnd() < 0.08;
    rows.push({
      id: `e${id++}`,
      src,
      dst,
      type,
      weight,
      validAt: T0 - 30 * DAY,
      invalidAt: retired ? T0 - 5 * DAY : null,
      srcFile,
    });
  };

  for (let fo = 0; fo < folders; fo++) {
    for (let fi = 0; fi < filesPerFolder; fi++) {
      // reading-order chain inside the document
      for (let s = 0; s + 1 < sections; s++) {
        push(node(fo, fi, s), node(fo, fi, s + 1), 'sequence', path(fo, fi), 1);
      }
      // intra-folder authored links (dense → a community per folder)
      for (let k = 0; k < crossPerFile; k++) {
        const other = Math.floor(rnd() * filesPerFolder);
        if (other === fi) continue;
        push(
          node(fo, fi, Math.floor(rnd() * sections)),
          node(fo, other, 0),
          rnd() < 0.5 ? 'wikilink' : 'pathcite',
          path(fo, fi),
          1 + Math.floor(rnd() * 3),
        );
      }
      // folder adjacency — the weak signal the percolation guard drops
      if (fi > 0) push(node(fo, fi, 0), node(fo, fi - 1, 0), 'same_dir', path(fo, fi), 1);
      // sparse cross-folder bridge
      if (rnd() < 0.05) {
        const fo2 = Math.floor(rnd() * folders);
        if (fo2 !== fo) push(node(fo, fi, 0), node(fo2, 0, 0), 'mdlink', path(fo, fi), 1);
      }
    }
  }
  return rows;
}

/** The full pipeline, as the view runs it. */
function runPipeline(rows, asOf = T0) {
  const g = internEdges(rows);
  const fg = contractSequence(g);
  fg.typeNamesRef = g.typeNames; // labelProp's same_dir guard reads this
  const comps = components(fg);
  const labels = labelProp(fg, comps);
  const { clusters, clusterOfFile } = rankClusters(g, fg, comps, labels, asOf);
  const bundles = buildBundles(fg, clusterOfFile, clusters.length);
  return { g, fg, comps, clusters, clusterOfFile, bundles };
}

/** Canonical, comparable shape — Maps and typed arrays don't JSON-compare. */
function canonical({ clusters, clusterOfFile, bundles }) {
  return JSON.stringify({
    clusters: clusters.map((c) => ({
      files: [...c.files],
      isElse: c.isElse,
      score: c.score,
      linkTotal: c.linkTotal,
      linkLive: c.linkLive,
      noteCount: c.noteCount,
      dirLabel: c.dirLabel,
      dirDupe: c.dirDupe,
      hubFile: c.hubFile,
    })),
    clusterOfFile: [...clusterOfFile],
    bundles: bundles.map((b) => [b.a, b.b, b.count ?? null]),
  });
}

// Scales chosen to straddle the interesting boundaries: fewer folders than
// the cluster cap, exactly the cap, and far more than it.
const SCALES = [
  { name: 'small vault (8 folders)', folders: 8, filesPerFolder: 6 },
  { name: 'at the cluster cap (12 folders)', folders: 12, filesPerFolder: 10 },
  { name: 'well past the cap (40 folders)', folders: 40, filesPerFolder: 15 },
  { name: 'large vault (120 folders)', folders: 120, filesPerFolder: 20 },
];

for (const scale of SCALES) {
  test(`bounded to ${MAP_TOP_CLUSTERS} + 1 groups — ${scale.name}`, () => {
    const rows = makeVault(scale);
    const out = runPipeline(rows);

    assert.ok(rows.length > 0, 'fixture produced no links');
    assert.ok(
      out.clusters.length <= MAP_TOP_CLUSTERS + 1,
      `${rows.length} links produced ${out.clusters.length} groups, cap is ${MAP_TOP_CLUSTERS + 1}`,
    );
    // At most one pooled "everything else" body.
    assert.ok(out.clusters.filter((c) => c.isElse).length <= 1, 'more than one "everything else" body');
  });

  test(`every file lands in exactly one group — ${scale.name}`, () => {
    const rows = makeVault(scale);
    const { fg, clusters, clusterOfFile } = runPipeline(rows);

    assert.equal(clusterOfFile.length, fg.F);
    for (let f = 0; f < fg.F; f++) {
      assert.ok(clusterOfFile[f] >= 0, `file-node ${f} was assigned to no group`);
      assert.ok(clusterOfFile[f] < clusters.length, `file-node ${f} points past the group list`);
    }
    // Membership lists and the index agree in both directions.
    const seen = new Set();
    clusters.forEach((c, ci) => {
      for (const f of c.files) {
        assert.equal(clusterOfFile[f], ci, `file-node ${f} is in group ${ci}'s list but indexed elsewhere`);
        assert.ok(!seen.has(f), `file-node ${f} appears in two groups`);
        seen.add(f);
      }
    });
    assert.equal(seen.size, fg.F, 'group membership does not cover every file-node');
  });

  test(`no fetched link is silently dropped — ${scale.name}`, () => {
    const rows = makeVault(scale);
    const { fg } = runPipeline(rows);

    // The honesty row's stated invariant, from the contractSequence comment:
    // fetched = Σ merged-link constituents + selfLoops.
    const constituents = fg.fEdges.reduce((acc, e) => acc + e.length, 0);
    assert.equal(
      constituents + fg.selfLoops,
      rows.length,
      'links vanished between the fetch and the merged graph',
    );
  });

  test(`identical input yields an identical partition — ${scale.name}`, () => {
    const a = runPipeline(makeVault(scale));
    const b = runPipeline(makeVault(scale));
    assert.equal(canonical(a), canonical(b), 'pipeline output is not deterministic');
  });

  test(`bundles stay within the pairwise bound — ${scale.name}`, () => {
    const rows = makeVault(scale);
    const { clusters, bundles } = runPipeline(rows);
    const c = clusters.length;
    assert.ok(
      bundles.length <= (c * (c - 1)) / 2,
      `${bundles.length} bundles exceeds the C·(C−1)/2 = ${(c * (c - 1)) / 2} bound for ${c} groups`,
    );
  });
}

test('a retired link is counted but not scored as live', () => {
  const rows = [
    { id: 'a', src: 'n1', dst: 'n2', type: 'wikilink', weight: 5, validAt: T0 - DAY, invalidAt: null, srcFile: 'x/a.md' },
    { id: 'b', src: 'n2', dst: 'n3', type: 'wikilink', weight: 7, validAt: T0 - DAY, invalidAt: T0 - 1, srcFile: 'x/b.md' },
    { id: 'c', src: 'n3', dst: 'n1', type: 'wikilink', weight: 2, validAt: T0 - DAY, invalidAt: null, srcFile: 'x/c.md' },
  ];
  const { clusters } = runPipeline(rows, T0);
  const total = clusters.reduce((acc, c) => acc + c.linkTotal, 0);
  const live = clusters.reduce((acc, c) => acc + c.linkLive, 0);
  assert.equal(total, 3, 'all three links should be counted');
  assert.equal(live, 2, 'the retired link should not count as live');
});

test('as-of replay: querying before a link was retired sees it live', () => {
  const rows = [
    { id: 'a', src: 'n1', dst: 'n2', type: 'wikilink', weight: 1, validAt: T0 - 10 * DAY, invalidAt: null, srcFile: 'x/a.md' },
    { id: 'b', src: 'n2', dst: 'n3', type: 'wikilink', weight: 1, validAt: T0 - 10 * DAY, invalidAt: T0 - 2 * DAY, srcFile: 'x/b.md' },
  ];
  const now = runPipeline(rows, T0);
  const before = runPipeline(rows, T0 - 5 * DAY);
  const liveNow = now.clusters.reduce((acc, c) => acc + c.linkLive, 0);
  const liveBefore = before.clusters.reduce((acc, c) => acc + c.linkLive, 0);
  assert.equal(liveNow, 1, 'the retired link should be expired at the present instant');
  assert.equal(liveBefore, 2, 'replayed before its retirement, the link should be live again');
});

test('an empty graph produces no groups rather than throwing', () => {
  const { clusters, bundles } = runPipeline([]);
  assert.equal(clusters.length, 0);
  assert.equal(bundles.length, 0);
});
