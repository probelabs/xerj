// ============================================================
// Xerj Console — "does at least one brain exist?" probe
//
// The Second Brain dashboard only earns a nav entry once the engine
// actually holds a brain — a console pointed at an engine nobody ever
// ran `xerj brain <folder>` against should not advertise an empty
// dashboard. This module answers exactly that yes/no question.
//
// It wraps the same discovery pattern second-brain-api.js uses
// (`_cat/indices/.xerj-memory-*` — PREFIX wildcard only: this
// engine's index-pattern matcher silently returns empty for an infix
// wildcard like `.xerj-memory-*-edges`, live-verified 2026-07-30, so
// the `-edges` cut happens client-side).
//
// Honesty stance: `false` on ANY transport failure. An unreachable
// engine means we cannot claim a brain exists, so the nav entry stays
// hidden — the deep-link route still resolves (app.js never filters
// the route table, only the nav list), so nothing is lost.
//
// Results are cached per baseUrl with a short TTL, so:
//   - the boot probe + periodic re-probes don't hammer `_cat`,
//   - switching backend/base-URL naturally misses the cache and
//     re-probes the new target without an explicit invalidation hook.
// ============================================================

const TTL_MS = 30_000;

/** baseUrl → { at: epoch-ms, value: boolean } */
const cache = new Map();

/** Drop all cached answers (e.g. right after `xerj brain` finishes,
 *  or when settings change under us). Safe to call any time. */
export function invalidateBrainsProbe() {
  cache.clear();
}

/**
 * True iff the engine at `baseUrl` holds at least one brain
 * (a reserved `.xerj-memory-{brain}-edges` index).
 *
 * Never throws; returns false on transport failure, non-OK status,
 * or an empty/absent baseUrl.
 */
export async function sbBrainsPresent(baseUrl, signal) {
  const base = (baseUrl || '').replace(/\/+$/, '');
  if (!base) return false;

  const hit = cache.get(base);
  if (hit && Date.now() - hit.at < TTL_MS) return hit.value;

  let present = false;
  try {
    const r = await fetch(`${base}/_cat/indices/.xerj-memory-*`, {
      signal,
      headers: { accept: 'text/plain, application/json' },
    });
    // A wildcard matching nothing is 200 with an EMPTY body on this
    // engine (live-verified; only a concrete name 404s), which parses
    // to zero brains below. Any non-OK status (404 from an older build
    // without the pattern route, 5xx, auth) is treated as "cannot
    // prove a brain exists", so we do not claim one.
    if (r.ok) {
      const text = await r.text();
      present = parseBrainIndices(text).length > 0;
    }
  } catch {
    present = false; // engine down / CORS / abort — no claim
  }

  cache.set(base, { at: Date.now(), value: present });
  return present;
}

/**
 * Parse a `_cat/indices` body into the list of brain names. `_cat` on
 * this engine emits plain text lines (`health status NAME uuid …`);
 * tolerate a JSON array in case a later build adds format=json.
 * Exported for the node self-test.
 */
export function parseBrainIndices(text) {
  const trimmed = (text || '').trim();
  let names = [];
  if (trimmed.startsWith('[')) {
    try { names = JSON.parse(trimmed).map((i) => i.index).filter(Boolean); } catch { names = []; }
  } else if (trimmed) {
    names = trimmed.split('\n')
      .map((l) => l.trim().split(/\s+/)[2])
      .filter(Boolean);
  }
  return names
    .filter((n) => n.startsWith('.xerj-memory-') && n.endsWith('-edges'))
    .map((n) => n.slice('.xerj-memory-'.length, -'-edges'.length))
    .filter((b) => b.length > 0);
}
