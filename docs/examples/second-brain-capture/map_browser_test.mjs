// Browser verification of THE MAP against the live 9200 data plane.
// The server's embedded console bundle is stale; every /_xerj-console/
// static asset is intercepted and served from the working tree, so this
// exercises the REAL new code end-to-end (auth, live graph API, canvas).
import puppeteer from 'puppeteer';
import fs from 'node:fs';
import path from 'node:path';

const BASE = 'http://localhost:9200';
const TOKEN = process.argv[2];
const EMAIL = process.argv[4] || 'verify@example.com';
const OUT = process.argv[3] || '/tmp/mapshots';
const UX = '/home/claude/ai/xerj/xerj-ux';
fs.mkdirSync(OUT, { recursive: true });
const say = (s) => console.log(s);

const MIME = { '.js': 'application/javascript', '.css': 'text/css', '.html': 'text/html', '.svg': 'image/svg+xml' };

const browser = await puppeteer.launch({
  headless: 'new',
  executablePath: '/usr/bin/google-chrome',
  args: ['--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu'],
});
const page = await browser.newPage();
await page.setViewport({ width: 1500, height: 1000, deviceScaleFactor: 1.5 });
page.on('console', (m) => { if (m.type() === 'error') say('PAGE ERROR: ' + m.text().slice(0, 200)); });
page.on('response', (r) => { if (r.status() >= 400) say('HTTP ' + r.status() + ' ' + r.url()); });
page.on('pageerror', (e) => say('PAGE EXCEPTION: ' + String(e).slice(0, 300)));

// serve fresh UX from disk
await page.setRequestInterception(true);
page.on('request', (req) => {
  const u = new URL(req.url());
  if (u.origin === BASE && u.pathname.startsWith('/_xerj-console/') && !u.pathname.startsWith('/_xerj-console/api/')) {
    let rel = u.pathname.slice('/_xerj-console/'.length);
    if (rel === '' || rel === 'setup' || rel === 'login') rel = rel ? `${rel}.html` : 'index.html';
    const file = path.join(UX, rel);
    if (fs.existsSync(file) && fs.statSync(file).isFile()) {
      req.respond({
        status: 200,
        headers: { 'content-type': MIME[path.extname(file)] || 'application/octet-stream', 'cache-control': 'no-cache' },
        body: fs.readFileSync(file),
      });
      return;
    }
  }
  req.continue();
});

const cdp = await page.createCDPSession();
await cdp.send('WebAuthn.enable');
await cdp.send('WebAuthn.addVirtualAuthenticator', {
  options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true,
             hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
});

// ── claim (if needed) ──
const me = await page.goto(`${BASE}/_xerj-console/api/v1/me`, { waitUntil: 'networkidle0' });
if (me.status() === 401) {
  if (!TOKEN) { say('NEEDS TOKEN'); process.exit(1); }
  await page.evaluateOnNewDocument((em) => { window.__email = em; }, EMAIL);
  await page.goto(`${BASE}/_xerj-console/setup#token=${TOKEN}`, { waitUntil: 'networkidle0' });
  await new Promise((r) => setTimeout(r, 1200));
  await page.evaluate(() => {
    const v = { email: window.__email || 'verify@example.com', display: 'Verify', pk: 'demo-key' };
    for (const el of document.querySelectorAll('input')) {
      if (['hidden', 'submit', 'button'].includes(el.type)) continue;
      const k = ((el.name || '') + (el.id || '') + (el.placeholder || '')).toLowerCase();
      el.value = el.type === 'email' || k.includes('email') ? v.email
               : (k.includes('passkey') || k.includes('pkname')) ? v.pk : v.display;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await page.evaluate(() => {
    const b = [...document.querySelectorAll('button')].find((x) => /enrol|create|claim/i.test(x.innerText));
    b && b.click();
  });
  await new Promise((r) => setTimeout(r, 5000));
  say('after claim: ' + page.url());
  if (/setup/.test(page.url())) {
    say('CLAIM FAILED: ' + (await page.evaluate(() => document.body.innerText.slice(0, 300))));
    await browser.close(); process.exit(1);
  }
}

const mapState = () => page.evaluate(async () => {
  const m = await import('/_xerj-console/src/ux/brain-map.js');
  return m.sbMapState();
});
const shot = async (n) => { await page.screenshot({ path: `${OUT}/${n}.png` }); say('  shot ' + n); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// canvas click helper: hit-rect center in canvas coords → page coords
async function clickHit(pred, pick) {
  const st = await mapState();
  const cands = st.hits.filter(pred);
  const hit = pick ? pick(cands, st) : cands[0];
  if (!hit) { say('NO HIT for predicate'); return null; }
  const box = await (await page.$('[data-sb-map-canvas]')).boundingBox();
  let x; let y;
  if (hit.seg) { const [x1, y1, x2, y2] = hit.seg; x = box.x + (x1 + x2) / 2; y = box.y + (y1 + y2) / 2; }
  else { x = box.x + hit.x + hit.w / 2; y = box.y + hit.y + hit.h / 2; }
  await page.mouse.click(x, y);
  return hit;
}

// ── docs brain: clustered mode ──
await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=docs`, { waitUntil: 'networkidle0' });
await sleep(6000);
const dbg = await page.evaluate(async () => {
  const m = await import('/_xerj-console/src/ux/brain-map.js');
  const api = await import('/_xerj-console/src/data/second-brain-api.js');
  const snap = api.sbSnapshot();
  let mountErr = null;
  try { m.mountBrainMap(); } catch (e) { mountErr = String((e && e.stack) || e); }
  return { snapBrain: snap.brain, hasOv: !!snap.overview, edges: snap.overview && snap.overview.edges, baseUrl: snap.baseUrl, mapEl: !!document.querySelector('[data-sb-map]'), mount: !!document.querySelector('[data-sb-map-mount]'), waiting: !!document.querySelector('[data-sb-map-waiting]'), mountErr };
});
say('mount debug: ' + JSON.stringify(dbg));
await sleep(3000);
let st = await mapState();
say('docs map state: ' + JSON.stringify({ status: st.status, mode: st.mode, fileNodes: st.fileNodes, clusters: st.clusters, bundles: st.bundles, statusLine: st.statusLine, error: st.error }));
await page.evaluate(() => {
  const el = document.querySelector('[data-sb-map]');
  el && el.scrollIntoView({ block: 'start' });
  window.scrollBy(0, -80);
});
await sleep(600);
await shot('m1-helicopter');

// expand the rank-0 cluster
const chit = await clickHit((h) => h.kind === 'cluster' && h.ci === 0);
say('clicked cluster: ' + JSON.stringify(chit && { ci: chit.ci }));
await sleep(900); // 240ms anim + name hydration
st = await mapState();
say('after expand: ' + JSON.stringify({ expanded: st.expanded, spread: st.spread, sel: st.sel }));
await shot('m2-expanded');

// select a member
const mhit = await clickHit((h) => h.kind === 'member' && !h.seg, (cands, st) => {
  // farthest labelled member from the expanded centroid, clear of other text
  const texts = st.hits.filter((h) => !h.seg && h.kind !== 'member');
  const free = cands.filter((c) => c.w > 20 && !texts.some((t) => c.x < t.x + t.w && t.x < c.x + c.w && c.y < t.y + t.h && t.y < c.y + c.h));
  return free[free.length - 1] || cands[cands.length - 1];
});
await sleep(700);
st = await mapState();
say('member sel: ' + JSON.stringify(st.sel));
await shot('m3-member-rail');
const railText = await page.evaluate(() => {
  const el = document.querySelector('[data-sb-map-rail]');
  return el ? el.innerText.slice(0, 600) : '(no rail)';
});
say('RAIL:\n' + railText);

// explore 2 hops
const expBtn = await page.$('[data-sb-map-explore]');
if (expBtn) {
  await expBtn.click();
  await sleep(1500);
  say('EXPLORE RAIL:\n' + await page.evaluate(() => document.querySelector('[data-sb-map-rail]').innerText.slice(0, 700)));
  await shot('m4-explore');
}

// fold away, then open a bundle line
await page.click('[data-sb-map-collapse]').catch(() => {});
await sleep(600);
const bhit = await clickHit((h) => h.kind === 'bundle', (cands, st) => {
  const texts = st.hits.filter((h) => !h.seg);
  return cands.find((c) => {
    const [x1, y1, , y2] = c.seg;
    const mx = (x1 + c.seg[2]) / 2; const my = (y1 + y2) / 2;
    return !texts.some((t) => mx >= t.x && mx <= t.x + t.w && my >= t.y && my <= t.y + t.h);
  }) || cands[0];
});
await sleep(1200);
st = await mapState();
say('bundle sel: ' + JSON.stringify(st.sel));
await shot('m5-bundle-evidence');
say('BUNDLE RAIL:\n' + await page.evaluate(() => document.querySelector('[data-sb-map-rail]').innerText.slice(0, 800)));

// belief-time preview event (the shared caret contract)
await page.evaluate(() => {
  document.dispatchEvent(new CustomEvent('sb:asof-preview', { detail: { ms: Date.now() - 400 * 864e5 } }));
});
await sleep(400);
await shot('m6-asof-preview');
await page.evaluate(() => {
  document.dispatchEvent(new CustomEvent('sb:asof-commit', { detail: { ms: null } }));
});

// FIND highlight
await page.evaluate(() => {
  const inp = document.querySelector('[data-sb-find]');
  if (inp) {
    inp.value = 'wordpress';
    inp.dispatchEvent(new Event('input', { bubbles: true }));
  }
});
await sleep(1500);
await shot('m7-find-highlight');

// ── notes brain: direct mode ──
await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=notes`, { waitUntil: 'networkidle0' });
await sleep(5000);
st = await mapState();
say('notes map state: ' + JSON.stringify({ status: st.status, mode: st.mode, fileNodes: st.fileNodes, statusLine: st.statusLine }));
await page.evaluate(() => {
  const el = document.querySelector('[data-sb-map]');
  el && el.scrollIntoView({ block: 'start' });
  window.scrollBy(0, -80);
});
await sleep(600);
await shot('m8-direct-mode');

// ── gtm brain: 10k links, PDF-heavy, 13 groups, retired links ──
await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=gtm`, { waitUntil: 'networkidle0' });
await sleep(8000);
st = await mapState();
say('gtm map state: ' + JSON.stringify({ status: st.status, mode: st.mode, fileNodes: st.fileNodes, clusters: st.clusters, bundles: st.bundles, statusLine: st.statusLine, error: st.error }));
await page.evaluate(() => {
  const el = document.querySelector('[data-sb-map]');
  el && el.scrollIntoView({ block: 'start' });
  window.scrollBy(0, -80);
});
await sleep(600);
await shot('m9-gtm-helicopter');
await clickHit((h) => h.kind === 'cluster' && h.ci === 0);
await sleep(1200);
await shot('m10-gtm-expanded');
say('gtm rail: ' + (await page.evaluate(() => document.querySelector('[data-sb-map-rail]').innerText.slice(0, 400))));

await browser.close();
say('done');
