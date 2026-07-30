// Judge THE MAP against the success criterion on the large `repo` brain.
// Drives the REAL console on :9200 (virtual WebAuthn), serves fresh UX
// assets from the working tree, screenshots every altitude, and measures
// drawn-element counts + frame timing. Run from /home/claude/ai/xerj.
import puppeteer from 'puppeteer';
import fs from 'node:fs';
import path from 'node:path';

const BASE = 'http://localhost:9200';
const BRAIN = process.argv[2] || 'repo';
const OUT = process.argv[3] || '/tmp/claude-1001/-home-claude-ai-xerj/e36b8c4c-3a94-4a76-80f1-2119380319c6/scratchpad/graphshots';
const TOKEN = process.argv[4] || null;
const UX = '/home/claude/ai/xerj/xerj-ux';
fs.mkdirSync(OUT, { recursive: true });
const say = (s) => console.log(s);

const MIME = { '.js': 'application/javascript', '.css': 'text/css', '.html': 'text/html', '.svg': 'image/svg+xml' };
const browser = await puppeteer.launch({
  headless: 'new', executablePath: '/usr/bin/google-chrome',
  args: ['--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu'],
});
const page = await browser.newPage();
await page.setViewport({ width: 1500, height: 1000, deviceScaleFactor: 1.5 });
page.on('pageerror', (e) => say('PAGE EXCEPTION: ' + String(e).slice(0, 300)));
page.on('console', (m) => { if (m.type() === 'error') say('PAGE ERROR: ' + m.text().slice(0, 200)); });
page.on('response', (r) => { if (r.status() >= 400) say('HTTP ' + r.status() + ' ' + r.url().slice(0, 140)); });

await page.setRequestInterception(true);
page.on('request', (req) => {
  const u = new URL(req.url());
  if (u.origin === BASE && u.pathname.startsWith('/_xerj-console/') && !u.pathname.startsWith('/_xerj-console/api/')) {
    let rel = u.pathname.slice('/_xerj-console/'.length);
    if (rel === '' || rel === 'setup' || rel === 'login') rel = rel ? `${rel}.html` : 'index.html';
    const file = path.join(UX, rel);
    if (fs.existsSync(file) && fs.statSync(file).isFile()) {
      req.respond({ status: 200, headers: { 'content-type': MIME[path.extname(file)] || 'application/octet-stream', 'cache-control': 'no-cache' }, body: fs.readFileSync(file) });
      return;
    }
  }
  req.continue();
});

const cdp = await page.createCDPSession();
await cdp.send('WebAuthn.enable');
await cdp.send('WebAuthn.addVirtualAuthenticator', {
  options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
});

const me = await page.goto(`${BASE}/_xerj-console/api/v1/me`, { waitUntil: 'networkidle0' });
if (me.status() === 401) {
  if (!TOKEN) { say('NEEDS TOKEN'); process.exit(1); }
  await page.goto(`${BASE}/_xerj-console/setup#token=${TOKEN}`, { waitUntil: 'networkidle0' });
  await new Promise((r) => setTimeout(r, 1200));
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('input')) {
      if (['hidden', 'submit', 'button'].includes(el.type)) continue;
      const k = ((el.name || '') + (el.id || '') + (el.placeholder || '')).toLowerCase();
      el.value = el.type === 'email' || k.includes('email') ? `graph-${Date.now()}@example.com`
        : (k.includes('passkey') || k.includes('pkname')) ? 'demo-key' : 'Verify';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await page.evaluate(() => {
    const b = [...document.querySelectorAll('button')].find((x) => /enrol|create|claim/i.test(x.innerText));
    b && b.click();
  });
  await new Promise((r) => setTimeout(r, 5000));
  if (/setup/.test(page.url())) { say('CLAIM FAILED: ' + (await page.evaluate(() => document.body.innerText.slice(0, 300)))); await browser.close(); process.exit(1); }
  say('claimed ok → ' + page.url());
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mapState = () => page.evaluate(async () => (await import('/_xerj-console/src/ux/brain-map.js')).sbMapState());
const shot = async (n) => { await page.screenshot({ path: `${OUT}/${n}.png` }); say('  shot ' + n); };
const scrollMap = () => page.evaluate(() => {
  const el = document.querySelector('[data-sb-map]');
  el && el.scrollIntoView({ block: 'start' }); window.scrollBy(0, -70);
});
const railText = (n = 800) => page.evaluate((k) => {
  const el = document.querySelector('[data-sb-map-rail]');
  return el ? el.innerText.slice(0, k) : '(no rail)';
}, n);

// frame-interval sampler around an interaction
const startFrames = () => page.evaluate(() => {
  window.__frames = []; window.__fon = true;
  let last = performance.now();
  const tick = (t) => { window.__frames.push(t - last); last = t; if (window.__fon) requestAnimationFrame(tick); };
  requestAnimationFrame(tick);
});
const stopFrames = () => page.evaluate(() => {
  window.__fon = false;
  const f = window.__frames.slice(1);
  if (!f.length) return null;
  const s = [...f].sort((a, b) => a - b);
  return { n: f.length, mean: f.reduce((a, b) => a + b, 0) / f.length, p95: s[Math.floor(s.length * 0.95)], max: s[s.length - 1] };
});

async function clickHit(pred, pick) {
  const st = await mapState();
  const cands = st.hits.filter(pred);
  const hit = pick ? pick(cands, st) : cands[0];
  if (!hit) { say('NO HIT'); return null; }
  const box = await (await page.$('[data-sb-map-canvas]')).boundingBox();
  let x; let y;
  if (hit.seg) { const [x1, y1, x2, y2] = hit.seg; x = box.x + (x1 + x2) / 2; y = box.y + (y1 + y2) / 2; }
  else { x = box.x + hit.x + hit.w / 2; y = box.y + hit.y + hit.h / 2; }
  await page.mouse.click(x, y);
  return hit;
}

// ── (a) helicopter default ──
const t0 = Date.now();
await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=${BRAIN}`, { waitUntil: 'networkidle0' });
let readyAt=null;
for (let i = 0; i < 240; i++) { const st = await mapState().catch(() => null); if (st && st.status === 'ready') { readyAt = Date.now(); break; } await sleep(1000); }
say('map ready wall-clock: ' + (readyAt ? ((readyAt - t0) + 'ms after navigation') : 'NEVER within 240s'));
let st = await mapState();
say(`READY in ~${Date.now() - t0}ms wall: ` + JSON.stringify({ status: st.status, mode: st.mode, fileNodes: st.fileNodes, clusters: st.clusters, bundles: st.bundles, statusLine: st.statusLine, error: st.error }));
await scrollMap(); await sleep(800);
st = await mapState();
say('HELICOPTER drawn hits: ' + JSON.stringify({
  clusters: st.hits.filter((h) => h.kind === 'cluster').length,
  bundleTraces: st.hits.filter((h) => h.kind === 'bundle').length,
  members: st.hits.filter((h) => h.kind === 'member').length,
}));
await shot('a-helicopter-default');
say('HOME RAIL:\n' + await railText(700));

// ── (b) one drill-down step: expand the biggest group ──
await startFrames();
await clickHit((h) => h.kind === 'cluster' && h.ci === 0);
await sleep(1400);
const fr1 = await stopFrames();
st = await mapState();
say('EXPANDED: ' + JSON.stringify({ expanded: st.expanded, spread: st.spread, members: st.hits.filter((h) => h.kind === 'member').length, satellite: st.hits.filter((h) => h.kind === 'satellite').length }));
say('expand frames: ' + JSON.stringify(fr1));
await shot('b-drilldown-expanded');
say('CLUSTER RAIL:\n' + await railText(700));

// ── (c) deepest: member → ledger + explore walks ──
const mhit = await clickHit((h) => h.kind === 'member' && !h.seg, (cands, s) => {
  const texts = s.hits.filter((h) => !h.seg && h.kind !== 'member');
  const free = cands.filter((c) => c.w > 20 && !texts.some((t) => c.x < t.x + t.w && t.x < c.x + c.w && c.y < t.y + t.h && t.y < c.y + c.h));
  return free[Math.floor(free.length / 2)] || cands[Math.floor(cands.length / 2)];
});
await sleep(800);
say('member sel: ' + JSON.stringify((await mapState()).sel));
// two explore walks = 4 hops honest depth
for (let w = 0; w < 2; w++) {
  const tw = Date.now();
  const clicked = await page.evaluate(() => {
    const b = document.querySelector('[data-sb-map-explore]');
    if (!b) return false;
    b.click();
    return true;
  });
  if (!clicked) { say('no explore btn'); break; }
  await sleep(150); // let the WALKING state render
  for (let i = 0; i < 40; i++) { const r = await railText(2000); if (!/WALKING…/.test(r)) break; await sleep(250); }
  say(`walk ${w + 1} settled in ~${Date.now() - tw}ms`);
}
say('EXPLORE RAIL:\n' + await railText(700));
await shot('c1-deepest-explore');

// satellite depth: expand the pooled "everything else" group (largest
// member count) and open its satellite list
await page.click('[data-sb-map-collapse]').catch(() => {});
await sleep(900);
let est = await mapState();
const lastCi = est.hits.filter((h) => h.kind === 'cluster').length - 1;
await clickHit((h) => h.kind === 'cluster' && h.ci === lastCi);
await sleep(1600);
est = await mapState();
say('ELSE EXPANDED: ' + JSON.stringify({ expanded: est.expanded, members: est.hits.filter((h) => h.kind === 'member').length, satellite: est.hits.filter((h) => h.kind === 'satellite').length }));
say('ELSE RAIL:\n' + await railText(500));
// the satellite hangs below the spiral — pan it into the canvas first
let satHit = (await mapState()).hits.find((h) => h.kind === 'satellite');
if (satHit) {
  const cbox = await (await page.$('[data-sb-map-canvas]')).boundingBox();
  const cx = satHit.x + satHit.w / 2; const cy = satHit.y + satHit.h / 2;
  if (cy > cbox.height - 30 || cy < 10 || cx < 10 || cx > cbox.width - 10) {
    const dx = (cbox.width / 2) - cx; const dy = (cbox.height / 2) - cy;
    await page.mouse.move(cbox.x + 8, cbox.y + 8);
    await page.mouse.down();
    await page.mouse.move(cbox.x + 8 + dx, cbox.y + 8 + dy, { steps: 12 });
    await page.mouse.up();
    await sleep(400);
  }
}
const sat = await clickHit((h) => h.kind === 'satellite');
if (sat) {
  await sleep(1200);
  say('SATELLITE RAIL:\n' + await railText(600));
  await shot('c3-satellite-list');
} else { say('NO SATELLITE on else-cluster'); await shot('c3-else-expanded'); }

// then the true leaf: open the ledger
await page.evaluate(() => { const b = document.querySelector('[data-sb-map-open]'); b && b.click(); });
await sleep(2500);
await shot('c2-leaf-ledger');

// ── (e) evidence quote open: bundle inspector with quotes ──
await scrollMap(); await sleep(400);
await page.click('[data-sb-map-collapse]').catch(() => {});
await sleep(900);
await clickHit((h) => h.kind === 'bundle', (cands, s) => {
  const texts = s.hits.filter((h) => !h.seg);
  return cands.find((c) => {
    const [x1, y1, x2, y2] = c.seg;
    const mx = (x1 + x2) / 2; const my = (y1 + y2) / 2;
    return !texts.some((t) => mx >= t.x && mx <= t.x + t.w && my >= t.y && my <= t.y + t.h);
  }) || cands[0];
});
await sleep(1500);
say('BUNDLE RAIL:\n' + await railText(900));
await shot('e-bundle-evidence-quotes');

// ── (d) statistics row ──
await page.evaluate(() => {
  const el = document.querySelector('[data-sb-body="edgesLive"]') || document.querySelector('[data-sb-body="scrub"]');
  el && el.scrollIntoView({ block: 'start' }); window.scrollBy(0, -60);
});
await sleep(600);
await shot('d-statistics');
say('READS PANEL:\n' + await page.evaluate(() => {
  const el = document.querySelector('[data-sb-body="reads"]');
  return el ? el.innerText.slice(0, 700) : '(none)';
}));
say('CROSSINGS PANEL:\n' + await page.evaluate(() => {
  const el = document.querySelector('[data-sb-body="crossings"]');
  return el ? el.innerText.slice(0, 500) : '(none)';
}));

// pan/zoom responsiveness at helicopter altitude
await scrollMap(); await sleep(300);
await startFrames();
const canvasEl = await page.$('[data-sb-map-canvas]');
if (!canvasEl) { say('NO CANVAS for pan/zoom'); await browser.close(); process.exit(1); }
const box = await canvasEl.boundingBox();
await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
for (let i = 0; i < 6; i++) await page.mouse.wheel({ deltaY: -120 });
await page.mouse.down();
await page.mouse.move(box.x + box.width / 2 + 200, box.y + box.height / 2 + 80, { steps: 20 });
await page.mouse.up();
for (let i = 0; i < 6; i++) await page.mouse.wheel({ deltaY: 120 });
await sleep(200);
say('pan/zoom frames: ' + JSON.stringify(await stopFrames()));

// language & honesty audit on real rendered text
const audit = await page.evaluate(() => {
  const txt = document.body.innerText;
  const bad = [];
  for (const w of [/\bedge\b/i, /\bsrc\b/i, /\bdst\b/i, /valid_at/i, /as_of/i, /\bas-of\b/i, /graph database/i]) {
    const m = txt.match(w);
    if (m) {
      const i = txt.indexOf(m[0]);
      bad.push({ word: m[0], ctx: txt.slice(Math.max(0, i - 60), i + 60).replace(/\n/g, ' ') });
    }
  }
  return bad;
});
say('LANGUAGE AUDIT (rendered text): ' + JSON.stringify(audit, null, 1));

await browser.close();
say('done');
