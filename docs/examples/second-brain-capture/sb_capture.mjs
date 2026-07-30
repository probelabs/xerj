// One session: claim the console, then capture every second-brain state:
// dashboard, the ledger, evidence quotes expanded, and time-travel after a retire.
import puppeteer from 'puppeteer';
import fs from 'node:fs';

const BASE = process.argv[2], TOKEN = process.argv[3], OUT = process.argv[4];
const say = (s) => console.log(s);
fs.mkdirSync(OUT, { recursive: true });

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

// ── claim ──
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
say('after claim: ' + page.url());
if (/setup/.test(page.url())) {
  say('CLAIM FAILED: ' + (await page.evaluate(() => document.body.innerText.slice(0, 300))));
  await browser.close(); process.exit(1);
}

const go = async () => {
  await page.goto(`${BASE}/_xerj-console/#/second-brain?brain=notes`, { waitUntil: 'networkidle0' });
  await new Promise((r) => setTimeout(r, 6500));
};
const shot = async (n, o = {}) => { await page.screenshot({ path: `${OUT}/${n}.png`, ...o }); say('  shot ' + n); };
const ledgerText = () => page.evaluate(() => {
  const el = [...document.querySelectorAll('*')].find((n) => (n.innerText || '').includes('THE LEDGER · ONE NOTE'));
  return el ? el.innerText.slice(0, 2500) : '(not found)';
});

// ── 1. dashboard top ──
await go();
await shot('01-dashboard-top');

// ── 2. the ledger, scrolled into view ──
await page.evaluate(() => {
  const el = [...document.querySelectorAll('*')].reverse()
    .find((n) => (n.innerText || '').includes('THE LEDGER · ONE NOTE') && n.children.length < 40);
  (el || document.body).scrollIntoView({ block: 'start' });
  window.scrollBy(0, -60);
});
await new Promise((r) => setTimeout(r, 900));
await shot('02-ledger');

// ── 3. expand evidence quotes ──
const n = await page.evaluate(() => {
  const t = [...document.querySelectorAll('button,a,span,div')]
    .filter((x) => ['QUOTE', 'WHY'].includes((x.innerText || '').trim()) && x.children.length === 0);
  t.slice(0, 6).forEach((x) => x.click());
  return t.length;
});
say('evidence toggles: ' + n);
await new Promise((r) => setTimeout(r, 1400));
await shot('03-evidence');
fs.writeFileSync(`${OUT}/ledger-evidence.txt`, await ledgerText());

// ── 4. full page (everything) ──
await shot('04-full', { fullPage: true });

await browser.close();
say('done');
