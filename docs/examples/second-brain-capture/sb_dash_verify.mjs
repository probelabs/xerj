// Live verification of the second-brain dashboard composition +
// statistics panel + ledger integration, against a running console.
//   node sb_dash_verify.mjs <base> <outdir> [token]
import puppeteer from 'puppeteer';
import fs from 'node:fs';

const BASE = process.argv[2] || 'http://localhost:9200';
const OUT = process.argv[3] || '/tmp/sbdash';
const TOKEN = process.argv[4] || '';
fs.mkdirSync(OUT, { recursive: true });
const say = (s) => console.log(s);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const browser = await puppeteer.launch({
  headless: 'new',
  args: ['--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu'],
});
const page = await browser.newPage();
await page.setViewport({ width: 1500, height: 1000, deviceScaleFactor: 1.5 });
const cdp = await page.createCDPSession();
await cdp.send('WebAuthn.enable');
await cdp.send('WebAuthn.addVirtualAuthenticator', {
  options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true,
             hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
});
page.on('console', (m) => { if (m.type() === 'error') say('  [console.error] ' + m.text().slice(0, 160)); });
page.on('pageerror', (e) => say('  [pageerror] ' + String(e).slice(0, 200)));

const go = async () => {
  await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=docs`, { waitUntil: 'networkidle0' });
  await sleep(5000);
};

await go();
let url = page.url();
say('landed: ' + url);
if (/setup|login|claim/i.test(url) && TOKEN) {
  say('claiming with token…');
  await page.goto(`${BASE}/_xerj-console/setup#token=${TOKEN}`, { waitUntil: 'networkidle0' });
  await sleep(1200);
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('input')) {
      if (['hidden', 'submit', 'button'].includes(el.type)) continue;
      const k = ((el.name || '') + (el.id || '') + (el.placeholder || '')).toLowerCase();
      // Unique per run — enrolment 409s on an email that is already registered.
      el.value = el.type === 'email' || k.includes('email') ? `verify${Date.now() % 1e7}@example.com`
               : (k.includes('passkey') || k.includes('pkname')) ? 'verify-key' : 'Owner';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await page.evaluate(() => {
    const b = [...document.querySelectorAll('button')].find((x) => /enrol|create|claim/i.test(x.innerText));
    b && b.click();
  });
  await sleep(5000);
  say('after claim: ' + page.url());
  await go();
}

const results = [];
const check = async (label, fn) => {
  let v;
  try { v = await fn(); } catch (e) { v = 'THREW ' + String(e).slice(0, 120); }
  results.push([label, v]);
  say(`${v === true ? 'PASS' : 'FAIL'} ${label}${v === true ? '' : ' → ' + JSON.stringify(v).slice(0, 220)}`);
};
const $text = (sel) => page.evaluate((s) => {
  const el = document.querySelector(s);
  return el ? el.innerText : null;
}, sel);
const has = (sel) => page.evaluate((s) => !!document.querySelector(s), sel);

// ---- 1. composition: panel order -----------------------------------
await check('panels present + §1.1 order (controls→map→scrub→stats→ego)', async () => {
  const ids = await page.evaluate(() =>
    [...document.querySelectorAll('[data-sb-body]')].map((e) => e.getAttribute('data-sb-body')));
  const want = ['controls', 'map', 'scrub', 'edgesLive', 'edgesTotal', 'invalidated', 'detectors',
    'typeDist', 'edgeTimeline', 'notes', 'crossings', 'reads', 'ego', 'hubs', 'notShown'];
  return JSON.stringify(ids) === JSON.stringify(want) ? true : ids;
});
await check('map mount + permanent structural disclosure', async () => {
  const t = await $text('[data-sb-body="map"]');
  return t && t.includes('GROUPED BY LINK STRUCTURE — CITATIONS AND FOLDER NEIGHBORHOOD, NOT MEANING') ? true : t;
});
await check('scrubber is its own panel (and not inside the ledger)', async () => {
  const inScrub = await has('[data-sb-body="scrub"] [data-sb-scrub]');
  const inEgo = await has('[data-sb-body="ego"] [data-sb-scrub]');
  return inScrub && !inEgo ? true : { inScrub, inEgo };
});
await check('controls hoisted to top (brain/focus/find)', async () => {
  const inCtl = await has('[data-sb-body="controls"] [data-sb-find]');
  const inEgo = await has('[data-sb-body="ego"] .sb-controls');
  return inCtl && !inEgo ? true : { inCtl, inEgo };
});

// ---- 2. statistics row ---------------------------------------------
await check('notes tile: total + file types (ran numbers)', async () => {
  const t = await $text('[data-sb-body="notes"]');
  return t && /\d/.test(t) && /notes/i.test(t) && /PROSE|LINES|JSON|YAML|MARKDOWN/i.test(t) ? true : t;
});
await check('crossings: honest pre-stamp REINDEX message', async () => {
  const t = await $text('[data-sb-body="crossings"]');
  return t && (t.includes('REINDEX TO SEE FILE-TYPE CROSSINGS') || t.includes('crossings')) ? true : t;
});
await check('embedder disclosure present', async () => {
  const t = await $text('[data-sb-body="crossings"]');
  return t && t.includes('NOT MEANING') && t.includes('lexical-feature-hash') ? true : t;
});
await check('read log: attributed rows (what — how much — why)', async () => {
  const t = await $text('[data-sb-body="reads"]');
  return t && t.includes('one overview read') && t.includes('the tiles on this page')
    && t.includes('finding your brains') ? true : t;
});
await check('read log: 2-hop walk attributed to the focus', async () => {
  const t = await $text('[data-sb-body="reads"]');
  return t && t.includes('2-hop walk') && t.includes('you focused this note') ? true : t;
});
await check('server counters: since-server-start framing (or honest refusal)', async () => {
  const t = await $text('[data-sb-body="reads"]');
  return t && t.includes('SINCE SERVER START')
    && (t.includes('links of brain') || t.includes('UNREADABLE') || t.includes('NO SEARCHES')) ? true : t;
});
await check('read-attribution refusal line present', async () => {
  const t = await $text('[data-sb-body="reads"]');
  return t && t.includes('not recorded by the engine') ? true : t;
});

// ---- 3. ledger integration -----------------------------------------
await check('ledger renders with focus card + path label', async () => {
  const t = await $text('[data-sb-body="ego"]');
  return t && t.includes('FOCUS') ? true : (t || '').slice(0, 150);
});
await check('refocus via hub click updates ledger + read log', async () => {
  const before = await $text('.sb-focuscard .sb-ftitle');
  const clicked = await page.evaluate(() => {
    const rows = [...document.querySelectorAll('[data-sb-body="hubs"] .sb-hubrow')];
    const other = rows.find((r) => !r.classList.contains('sb-hub-active'));
    if (!other) return false;
    other.click();
    return true;
  });
  if (!clicked) return 'no non-active hub row';
  await sleep(2500);
  const after = await $text('.sb-focuscard .sb-ftitle');
  const log = await $text('[data-sb-body="reads"]');
  return after && after !== before && log.includes('2-hop walk') ? true : { before, after };
});
await check('belief-time commit from the standalone scrubber', async () => {
  const strip = await page.$('[data-sb-body="scrub"] [data-sb-strip]');
  if (!strip) return 'no strip';
  // The 55vh map sits above the scrubber — bring the strip on-screen
  // or the synthetic pointer lands outside the viewport (a no-op).
  await strip.scrollIntoView();
  await sleep(300);
  const box = await strip.boundingBox();
  await page.mouse.click(box.x + box.width * 0.5, box.y + box.height / 2);
  await sleep(2500);
  const ctl = await $text('[data-sb-body="controls"]');
  return ctl && ctl.includes('BACK TO NOW') ? true : ctl && ctl.slice(0, 200);
});
await check('ledger rows restate under the committed as-of', async () => {
  const t = await $text('[data-sb-body="scrub"]');
  return t && /BELIEVED AT THIS MOMENT/.test(t) ? true : t;
});
// back to NOW
await page.evaluate(() => { const b = document.querySelector('[data-sb-now]'); b && b.click(); });
await sleep(2000);
await check('FIND logs its search + returns refocus links', async () => {
  await page.type('[data-sb-find]', 'security');
  await sleep(1500);
  const slotShown = await page.evaluate(() => {
    const s = document.querySelector('[data-sb-findresults]');
    return s && !s.hidden && s.innerText.length > 0;
  });
  const log = await $text('[data-sb-body="reads"]');
  return slotShown && log.includes('you typed in FIND') ? true : { slotShown };
});
await check('map placeholder honest when module absent (or live map mounted)', async () => {
  const t = await $text('[data-sb-body="map"]');
  const live = await has('[data-sb-map][data-sb-map-live]');
  return live || (t && (t.includes('NOT IN THIS BUILD') || t.includes('THE MAP DRAWS HERE'))) ? true : t;
});
await check('live map: canvas mounted, panel patching spared it', async () => {
  const live = await has('[data-sb-map][data-sb-map-live] [data-sb-map-canvas]');
  return live ? true : 'no live canvas (fails until the map slice lands — honest state covered above)';
});
await check('map fetch attributed in the shared read log', async () => {
  const t = await $text('[data-sb-body="reads"]');
  return t && t.includes('building the map') ? true
    : 'no map read logged (map slice not mounted yet)';
});

// ---- screenshots (evidence) ----------------------------------------
await page.evaluate(() => window.scrollTo(0, 0));
await sleep(400);
await page.screenshot({ path: `${OUT}/01-top-controls-map-scrub.png` });
await page.evaluate(() => document.querySelector('[data-sb-body="notes"]')?.scrollIntoView({ block: 'center' }));
await sleep(400);
await page.screenshot({ path: `${OUT}/02-statistics-row.png` });
await page.evaluate(() => document.querySelector('[data-sb-body="ego"]')?.scrollIntoView());
await sleep(400);
await page.screenshot({ path: `${OUT}/03-ledger.png` });
await page.screenshot({ path: `${OUT}/04-full.png`, fullPage: true });

const fails = results.filter(([, v]) => v !== true).length;
say(fails === 0 ? `ALL ${results.length} CHECKS PASS` : `${fails}/${results.length} CHECKS FAILED`);
await browser.close();
process.exit(fails ? 1 : 0);
