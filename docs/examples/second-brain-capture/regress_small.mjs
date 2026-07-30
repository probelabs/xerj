// Regression: docs (clustered, 435 links) + notes (direct mode) after
// the layout/view changes.
import puppeteer from 'puppeteer';
import fs from 'node:fs';
import path from 'node:path';
const BASE = 'http://localhost:9200';
const OUT = '/tmp/claude-1001/-home-claude-ai-xerj/e36b8c4c-3a94-4a76-80f1-2119380319c6/scratchpad/graphshots';
const UX = '/home/claude/ai/xerj/xerj-ux';
const say = (s) => console.log(s);
const MIME = { '.js': 'application/javascript', '.css': 'text/css', '.html': 'text/html', '.svg': 'image/svg+xml' };
const browser = await puppeteer.launch({ headless: 'new', executablePath: '/usr/bin/google-chrome', args: ['--no-sandbox', '--disable-dev-shm-usage'] });
const page = await browser.newPage();
await page.setViewport({ width: 1500, height: 1000, deviceScaleFactor: 1.5 });
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
await cdp.send('WebAuthn.addVirtualAuthenticator', { options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true } });
const TOKEN = process.argv[2];
const me = await page.goto(`${BASE}/_xerj-console/api/v1/me`, { waitUntil: 'networkidle0' });
if (me.status() === 401) {
  await page.goto(`${BASE}/_xerj-console/setup#token=${TOKEN}`, { waitUntil: 'networkidle0' });
  await new Promise((r) => setTimeout(r, 1200));
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('input')) {
      if (['hidden', 'submit', 'button'].includes(el.type)) continue;
      const k = ((el.name || '') + (el.id || '') + (el.placeholder || '')).toLowerCase();
      el.value = el.type === 'email' || k.includes('email') ? `rg-${Date.now()}@example.com` : (k.includes('passkey') || k.includes('pkname')) ? 'k' : 'V';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
    const b = [...document.querySelectorAll('button')].find((x) => /enrol|create|claim/i.test(x.innerText));
    b && b.click();
  });
  await new Promise((r) => setTimeout(r, 5000));
  say('claim → ' + page.url());
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mapState = () => page.evaluate(async () => (await import('/_xerj-console/src/ux/brain-map.js')).sbMapState());
for (const brain of ['docs', 'notes']) {
  await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=${brain}`, { waitUntil: 'networkidle0' });
  let st = null;
  for (let i = 0; i < 40; i++) { st = await mapState().catch(() => null); if (st && st.status === 'ready') break; await sleep(1000); }
  say(`${brain}: ` + JSON.stringify({ status: st && st.status, mode: st && st.mode, fileNodes: st && st.fileNodes, clusters: st && st.clusters, bundles: st && st.bundles, statusLine: st && st.statusLine, error: st && st.error }));
  await page.evaluate(() => { const el = document.querySelector('[data-sb-map]'); el && el.scrollIntoView({ block: 'start' }); window.scrollBy(0, -70); });
  await sleep(700);
  await page.screenshot({ path: `${OUT}/regress-${brain}.png` });
  say('  shot regress-' + brain);
}
await browser.close();
say('done');
