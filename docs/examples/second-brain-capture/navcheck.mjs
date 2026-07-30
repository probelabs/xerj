// Conditional-nav verification: does "Second Brain" appear in the nav?
// argv[2] = setup token (only needed on an unclaimed server)
// argv[3] = screenshot name
import puppeteer from 'puppeteer';
import fs from 'node:fs';
import path from 'node:path';

const BASE = 'http://localhost:9200';
const TOKEN = process.argv[2] && process.argv[2] !== '-' ? process.argv[2] : null;
const NAME = process.argv[3] || 'navcheck';
const OUT = '/tmp/claude-1001/-home-claude-ai-xerj/e36b8c4c-3a94-4a76-80f1-2119380319c6/scratchpad/graphshots';
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
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const me = await page.goto(`${BASE}/_xerj-console/api/v1/me`, { waitUntil: 'networkidle0' });
if (me.status() === 401) {
  if (!TOKEN) { say('NEEDS TOKEN'); process.exit(1); }
  await page.goto(`${BASE}/_xerj-console/setup#token=${TOKEN}`, { waitUntil: 'networkidle0' });
  await sleep(1200);
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('input')) {
      if (['hidden', 'submit', 'button'].includes(el.type)) continue;
      const k = ((el.name || '') + (el.id || '') + (el.placeholder || '')).toLowerCase();
      el.value = el.type === 'email' || k.includes('email') ? 'nav-verify@example.com'
        : (k.includes('passkey') || k.includes('pkname')) ? 'demo-key' : 'Verify';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await page.evaluate(() => {
    const b = [...document.querySelectorAll('button')].find((x) => /enrol|create|claim/i.test(x.innerText));
    b && b.click();
  });
  await sleep(5000);
  if (/setup/.test(page.url())) { say('CLAIM FAILED: ' + (await page.evaluate(() => document.body.innerText.slice(0, 300)))); await browser.close(); process.exit(1); }
}

await page.goto(`${BASE}/_xerj-console/#/dashboards`, { waitUntil: 'networkidle0' });
await sleep(6000); // probe tick + potential re-render
const navText = await page.evaluate(() => document.body.innerText);
const inNav = /second brain/i.test(navText);
say('NAV LISTS SECOND BRAIN: ' + inNav);
await page.screenshot({ path: `${OUT}/${NAME}.png` });
say('shot ' + NAME);
// deep-link must still resolve even when nav hides it
await page.goto(`${BASE}/_xerj-console/#/second-brain`, { waitUntil: 'networkidle0' });
await sleep(4000);
const deep = await page.evaluate(() => document.body.innerText.slice(0, 1200));
say('DEEP LINK BODY HEAD:\n' + deep.slice(0, 600));
await page.screenshot({ path: `${OUT}/${NAME}-deeplink.png` });
say('shot ' + NAME + '-deeplink');
await browser.close();
