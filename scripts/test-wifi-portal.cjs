// Offline portal regression tests. Uses a mock device API, never a real board.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "../data/browser-check/node_modules/playwright");
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");
const root = path.resolve(__dirname, "../examples/esp32-rust-ota-client/crates/wifi-manager/web");

async function main() {
  const server = http.createServer((req, res) => {
    const pathname = new URL(req.url, "http://localhost").pathname;
    const filename = { "/": "index.html", "/status": "status.html", "/setup.css": "setup.css", "/setup.js": "setup.js" }[pathname];
    if (!filename) { res.writeHead(404); res.end(); return; }
    res.setHeader("Content-Type", filename.endsWith(".js") ? "text/javascript" : filename.endsWith(".css") ? "text/css" : "text/html");
    res.setHeader("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'");
    res.end(fs.readFileSync(path.join(root, filename)));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  let browser;
  try {
    browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHANNEL ? { channel: process.env.PLAYWRIGHT_CHANNEL } : {}) });
    const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
    const page = await context.newPage();
    page.setDefaultTimeout(8000);
    const errors = [], posts = [];
    page.on("pageerror", error => errors.push(error.message));
    let state = {
      ap_name: "ESP32-Setup-123456", phase: "idle", message: "Choose Wi-Fi.",
      saved_ssid: "Previous Wi-Fi", target_ssid: null, ip_address: null,
      busy: false, scanning: false, scan_error: null, close_in: null, csrf: "test-only",
      networks: [{ ssid: "Home 2.4 GHz", rssi: -40, secured: true },
        { ssid: "<img src=x onerror=alert(1)>", rssi: -75, secured: false }]
    };
    let rejectConnect = false, failStatus = false;
    await page.route("**/api/**", async route => {
      const req = route.request(), endpoint = new URL(req.url()).pathname;
      if (endpoint === "/api/status") {
        if (failStatus) { await route.abort(); return; }
        await route.fulfill({ json: state }); return;
      }
      assert.equal(req.headers()["x-setup-token"], "test-only");
      assert.equal(req.headers()["content-type"], "application/json");
      posts.push({ endpoint, body: req.postDataJSON() });
      if (rejectConnect) {
        await route.fulfill({ status: 409, json: { error: "Please wait for the current attempt." } }); return;
      }
      if (endpoint === "/api/connect") {
        state = { ...state, busy: true, phase: "connecting", target_ssid: req.postDataJSON().ssid };
      }
      await route.fulfill({ status: 202, json: { accepted: true } });
    });
    const origin = "http://127.0.0.1:" + server.address().port;
    await page.goto(origin);
    await page.getByRole("button", { name: /Home 2.4 GHz/ }).waitFor();
    await page.locator("#ssid").fill("My hidden network");
    await page.locator("#password").fill("hidden-pass");
    assert.equal(await page.locator("#connect").isEnabled(), true);
    await page.locator("#ssid").fill("x".repeat(33));
    assert.equal(await page.locator("#connect").isDisabled(), true);
    await page.getByRole("button", { name: /Home 2.4 GHz/ }).click();
    assert.equal(await page.locator("#ssid").inputValue(), "Home 2.4 GHz");
    assert.equal(await page.locator("#connect").isDisabled(), true);
    await page.locator("#password").fill("short");
    assert.equal(await page.locator("#connect").isDisabled(), true);
    await page.locator("#password").fill(" password ");
    await page.locator("#scan").click();
    await page.waitForResponse(response => response.url().endsWith("/api/status"));
    assert.equal(await page.locator("#password").inputValue(), " password ");
    await page.locator("#show-password").click();
    assert.equal(await page.locator("#password").getAttribute("type"), "text");
    assert.equal(await page.locator("#connect").isEnabled(), true);
    assert.equal(await page.locator("#networks img").count(), 0);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: path.resolve(__dirname, "../data/wifi-setup-mobile.png"), fullPage: true });
    rejectConnect = true;
    await page.locator("#connect").click();
    await page.getByRole("alert").filter({ hasText: "Please wait" }).waitFor();
    assert.equal(await page.locator("#password").inputValue(), " password ");
    rejectConnect = false;
    await page.locator("#connect").click();
    await page.waitForURL("**/status");
    assert.deepEqual(posts.at(-1), { endpoint: "/api/connect", body: { ssid: "Home 2.4 GHz", password: " password " } });
    state = { ...state, busy: false, phase: "failed", message: "Check password. Previous settings are unchanged." };
    await page.getByRole("heading", { name: "Let's try that again." }).waitFor();
    assert.equal(state.saved_ssid, "Previous Wi-Fi");
    await page.locator("#edit-network").click();
    await page.getByRole("button", { name: /<img/ }).click();
    assert.equal(await page.locator("#open-network").isChecked(), true);
    assert.equal(await page.locator("#password").isDisabled(), true);
    assert.equal(await page.locator("#connect").isEnabled(), true);
    await page.locator("#connect").click();
    await page.waitForURL("**/status");
    assert.equal(posts.at(-1).body.password, "");
    state = { ...state, busy: false, phase: "connected", saved_ssid: state.target_ssid,
      ip_address: "10.0.0.42", message: "Wi-Fi connected. Settings are saved.", close_in: 20 };
    await page.getByRole("heading", { name: "You're connected." }).waitFor();
    assert.equal(await page.locator("#status-ip").textContent(), "10.0.0.42");
    assert.match(await page.locator("#closing-message").textContent(), /MQTT are checked separately/);
    await page.screenshot({ path: path.resolve(__dirname, "../data/wifi-setup-connected.png"), fullPage: true });
    failStatus = true;
    await page.getByRole("status").filter({ hasText: "Setup connection closed" }).waitFor();
    assert.equal(await page.locator("#retry").isDisabled(), true);
    assert.deepEqual(errors, []);
    console.log("PASS: mobile layout, secured/open/hidden entry, validation, XSS-safe SSIDs, server errors, failed retry, saved state, success and hotspot disconnect.");
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
