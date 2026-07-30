// Retire one link over the API, then screenshot the dashboard so the
// "RETIRED · KEPT FOR REPLAY" state is visible in the UI (claim 2).
import puppeteer from 'puppeteer';
import fs from 'node:fs';

const BASE = process.argv[2], TOKEN = process.argv[3], OUT = process.argv[4];
const say = (s) => console.log(s);
fs.mkdirSync(OUT, { recursive: true });

// 1. pick a live edge and retire it (plain HTTP, before the browser work)
const ov = await (await fetch(`${BASE}/_graph/notes/overview`)).json();
const hub = ov.hubs.out[0].id;
const ego = await (await fetch(`${BASE}/_graph/notes/ego?node=${hub}&hops=1`)).json();
const edge = ego.edges.find((e) => e.evidence && e.evidence.quote) || ego.edges[0];
const t = Date.now();
const del = await (await fetch(`${BASE}/_graph/notes/link/${edge.edge_id}?invalid_at=${t}`, { method: 'DELETE' })).json();
say('retired ' + edge.edge_id + ' -> ' + JSON.stringify(del));
const after = await (await fetch(`${BASE}/_graph/notes/overview`)).json();
say('overview now: ' + JSON.stringify(after.edges));
fs.writeFileSync(`${OUT}/retire.json`, JSON.stringify({ retired: edge.edge_id, del, edges: after.edges, hub }, null, 2));

// 2. claim console + screenshot the retired state
const browser = await puppeteer.launch({ headless: 'new', args: ['--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu'] });
const page = await browser.newPage();
await page.setViewport({ width: 1500, height: 1000, deviceScaleFactor: 1.5 });
const cdp = await page.createCDPSession();
await cdp.send('WebAuthn.enable');
await cdp.send('WebAuthn.addVirtualAuthenticator', {
  options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true,
             hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true } });
await page.goto(`${BASE}/_xerj-console/setup#token=${TOKEN}`, { waitUntil: 'networkidle0' });
await new Promise((r) => setTimeout(r, 1200));
await page.evaluate(() => {
  const v = { email: 'owner@example.com', display: 'Owner', pk: 'demo-key' };
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
if (/setup/.test(page.url())) { say('CLAIM FAILED'); await browser.close(); process.exit(1); }

await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=notes`, { waitUntil: 'networkidle0' });
await new Promise((r) => setTimeout(r, 6500));
await page.screenshot({ path: `${OUT}/05-retired-top.png` });
fs.writeFileSync(`${OUT}/retired.txt`, await page.evaluate(() => document.body.innerText.slice(0, 3000)));
// focus the node whose edge we retired, include retired rows
await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=notes&node=${process.argv[5] || ''}`, { waitUntil: 'networkidle0' }).catch(() => {});
await new Promise((r) => setTimeout(r, 4000));
await page.screenshot({ path: `${OUT}/06-retired-full.png`, fullPage: true });
await browser.close();
say('done');
