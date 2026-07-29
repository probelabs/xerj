// ============================================================
// Dashboard — SECOND BRAIN
//
// The relationship layer over documents that already exist: edges are
// ordinary XERJ documents, bi-temporal and soft-delete-only, asserted
// by deterministic detectors (or by hand) with the exact source quote
// as evidence. This dashboard is an EGO LEDGER — inbound edges left,
// the focus memory centre, outbound edges right — with a belief-time
// scrubber to replay what the brain believed at any moment. A global
// force-directed graph was rejected by design (non-deterministic
// layout, collapses past ~200 nodes); see ux/ego-ledger.js.
//
// LIVE DATA ONLY. data/backends/xerj.js routes this dashboard to
// data/second-brain-api.js, which reads /_graph/{brain}/overview and
// /_graph/{brain}/ego and NEVER falls back to mock — an empty or
// unreachable brain renders an honest state that says how to fill it
// (`xerj brain ~/your-folder`), not invented beliefs.
// ============================================================

import { renderPanelBody } from '../ux/ego-ledger.js';
import { sbAfterRender } from '../data/second-brain-api.js';

/** One panel body: renderPanelBody draws the inner HTML; the
 *  `data-sb-body` wrapper is the in-place re-render target the
 *  interaction controller patches after refocus / as-of commits. */
const body = (id, data) => `<div data-sb-body="${id}">${renderPanelBody(id, data)}</div>`;

export const secondBrain = {
  id: 'second-brain',
  name: 'Second Brain',
  render: ({ data, time }) => {
    // Traces (the 1px focus→group lines) need real layout; schedule a
    // post-paint measure+draw pass. No-op outside a browser.
    sbAfterRender();
    return {
      title: 'SECOND BRAIN',
      kicker: 'WHAT YOUR NOTES BELIEVE · EVERY LINK HAS A QUOTE · REPLAYABLE AT ANY MOMENT',
      meta: [time, 'XERJ-GRAPH'],
      // Eyebrows speak user words — link / believed / retired / taught.
      // Schema vocabulary (edge, src/dst, as-of) stays in the API and
      // in the evidence paper-trail, never in a panel title.
      panels: [
        { id: 'edgesLive', eyebrow: 'BELIEVED AT THIS MOMENT', cols: 3, type: 'metric',
          render: () => body('edgesLive', data) },
        { id: 'edgesTotal', eyebrow: 'EVER ASSERTED', cols: 3, type: 'metric',
          render: () => body('edgesTotal', data) },
        { id: 'invalidated', eyebrow: 'RETIRED · KEPT FOR REPLAY', cols: 3, type: 'metric',
          render: () => body('invalidated', data) },
        { id: 'detectors', eyebrow: 'WHAT TAUGHT THIS BRAIN', cols: 3, type: 'metric',
          render: () => body('detectors', data) },
        { id: 'typeDist', eyebrow: 'HOW NOTES CONNECT', cols: 6, type: 'dist',
          render: () => body('typeDist', data) },
        { id: 'edgeTimeline', eyebrow: 'NEW LINKS PER DAY', cols: 6, type: 'series',
          render: () => body('edgeTimeline', data) },
        { id: 'ego', eyebrow: 'THE LEDGER · ONE NOTE, EVERYTHING IT TOUCHES', cols: 12, type: 'ego',
          render: () => body('ego', data) },
        { id: 'hubs', eyebrow: 'CENTERS OF GRAVITY', cols: 6, type: 'topn',
          render: () => body('hubs', data) },
        { id: 'notShown', eyebrow: 'WHAT THIS VIEW DID NOT SHOW', cols: 6, type: 'honesty',
          render: () => body('notShown', data) },
      ],
    };
  },
};
