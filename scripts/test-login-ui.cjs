// Uses the private local handoff file; never prints credentials or writes traces.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {chromium} = require(process.env.PLAYWRIGHT_MODULE || '../data/browser-check/node_modules/playwright');
const root = path.resolve(__dirname, '..');
const origin = process.env.GATEWAY_TEST_URL;
if (!origin?.startsWith('https://')) throw new Error('Set GATEWAY_TEST_URL to the configured HTTPS origin.');

// RFC 6238 code from the private secret, matching python/gateway/totp.py.
// The gateway accepts each code once, so wait 30 s after any manual login.
function totpCode(secret) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  let bits = '';
  for (const char of secret.replace(/[\s=]/g, '').toUpperCase()) bits += alphabet.indexOf(char).toString(2).padStart(5, '0');
  const key = Buffer.from(bits.match(/.{8}/g).map(byte => parseInt(byte, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const digest = require('node:crypto').createHmac('sha1', key).update(counter).digest();
  const offset = digest[digest.length - 1] & 0x0f;
  return String((digest.readUInt32BE(offset) & 0x7fffffff) % 1000000).padStart(6, '0');
}

async function main() {
  const privateText = fs.readFileSync(path.join(root, 'data/private/dashboard-login.txt'), 'utf8');
  const username = privateText.match(/^Username: (.+)$/m)[1].trim();
  const password = privateText.match(/^Password: (.+)$/m)[1].trim();
  const auth = JSON.parse(fs.readFileSync(path.join(root, 'data/private/dashboard-auth.json'), 'utf8'));
  const browser = await chromium.launch({headless: true, channel: 'msedge'});
  try {
    const context = await browser.newContext({viewport: {width: 1365, height: 900}});
    const page = await context.newPage();
    const pageErrors = [];
    page.on('pageerror', error => pageErrors.push(error.message));
    assert.equal((await context.request.get(origin + '/api/ota/projects')).status(), 401);
    await page.goto(origin + '/');
    await page.waitForURL(origin + '/login');
    await page.setViewportSize({width: 390, height: 844});
    await page.screenshot({path: path.join(root, 'data/login-mobile.png'), fullPage: true});
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.setViewportSize({width: 1365, height: 900});
    await page.locator('#username').fill(username);
    await page.locator('#password').fill(password);
    await page.locator('#code').fill(totpCode(auth.totp_secret));
    await page.locator('#login-submit').click();
    await page.waitForURL(origin + '/');
    await page.locator('.topbar-actions button', {hasText: 'Log out'}).waitFor();
    assert.equal((await context.request.get(origin + '/sessions')).status(), 200);
    assert.equal((await context.request.get(origin + '/api/ota/status')).status(), 200);
    await page.locator('#nav-tinyml').click();
    await page.waitForFunction(() => !document.getElementById('tml-device').disabled);
    assert.equal(await page.locator('#viewer-access-panel').isVisible(), false);
    await page.waitForFunction(() => document.getElementById('tml-message').textContent.includes('connected'), null, {timeout: 15000});
    const monitoring = await context.request.get(origin + '/api/tinyml/status');
    assert.equal(monitoring.status(), 200);
    await page.locator('#nav-console').click();
    await page.waitForFunction(() => !document.getElementById('console-refresh').disabled);
    assert.equal(await page.locator('#viewer-access-panel').isVisible(), false);
    assert.equal(await page.locator('#console-level').isDisabled(), false);
    assert.equal(await page.locator('#console-filter').isDisabled(), false);
    const cookies = await context.cookies();
    const cookie = cookies.find(c => c.name === '__Host-dashboard');
    assert.ok(cookie && cookie.secure && cookie.httpOnly && cookie.sameSite === 'Strict');
    await page.locator('.topbar-actions button', {hasText: 'Log out'}).click();
    await page.waitForURL(origin + '/login');
    assert.equal((await context.request.get(origin + '/api/ota/projects')).status(), 401);
    assert.deepEqual(pageErrors, []);
    await context.close();
    console.log('PASS mobile login layout, real browser login/logout, CSV/OTA access, and shared Monitoring access.');
  } finally { await browser.close(); }
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });
