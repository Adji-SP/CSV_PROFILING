// Deployment refresh regression test. All APIs are fixtures; no board is contacted.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");
const root = path.resolve(__dirname, "../web");
const device = id => ({ device_id: id, name: id, chip: "esp32s3", status: "online", current_version: "1", ota_status: "idle", last_seen: new Date().toISOString() });
const firmware = (id, status = "ready") => ({ firmware_id: id, build_id: `build-${id}`, project: "test", target: "esp32s3", version: id, filename: `${id}.bin`, size: 100, sha256: "abcdef", status, created_at: new Date().toISOString(), download_url: `/firmware/${id}.bin` });

async function main() {
  const server = http.createServer((req, res) => {
    const name = new URL(req.url, "http://localhost").pathname;
    if (name === "/app-config.js") {
      res.setHeader("Content-Type", "text/javascript");
      res.end("window.APP_CONFIG = { otaApiBase: location.origin, apiBase: location.origin };");
      return;
    }
    const file = path.resolve(root, name === "/" ? "index.html" : `.${name}`);
    if (!file.startsWith(`${root}${path.sep}`)) { res.writeHead(403); res.end(); return; }
    fs.readFile(file, (error, data) => {
      if (error) { res.writeHead(404); res.end(); return; }
      res.setHeader("Content-Type", file.endsWith(".js") ? "text/javascript" : file.endsWith(".css") ? "text/css" : "text/html");
      res.end(data);
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  let browser;
  try {
    browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHANNEL ? { channel: process.env.PLAYWRIGHT_CHANNEL } : {}) });
    const page = await browser.newPage();
    page.setDefaultTimeout(8000);
    const errors = [], mutations = [];
    let devices = [device("old-board")], images = [firmware("old-image")], failure = "";
    page.on("pageerror", error => errors.push(error.message));
    await page.route("https://fonts.googleapis.com/**", route => route.abort());
    await page.route("**/sessions", route => route.fulfill({ json: [] }));
    await page.route("**/api/**", async route => {
      const p = new URL(route.request().url()).pathname;
      if (route.request().method() !== "GET") mutations.push(p);
      if (p === failure) return route.fulfill({ status: 503, json: { error: { message: "Deployment list unavailable" } } });
      const value = p === "/api/ota/devices" ? devices : p === "/api/ota/firmware" ? images : p.endsWith("/status") ? { status: "online" } : [];
      await route.fulfill({ json: value });
    });
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.locator("#nav-ota").click();
    await page.locator('[data-firmware-deploy="old-image"]').waitFor();
    await page.waitForFunction(() => document.getElementById("ota-summary-devices").textContent === "1");

    // Both caches are populated, then a board registers and a second build finishes.
    devices = [device("old-board"), device("new-board")];
    images = [firmware("old-image"), firmware("new-image"), firmware("deleted-image", "deleting")];
    await page.locator('[data-firmware-deploy="old-image"]').click();
    await page.locator("#deploy-overlay").waitFor({ state: "visible" });
    assert.deepEqual(await page.locator("#deploy-device option").evaluateAll(items => items.map(item => item.value)), ["old-board", "new-board"]);
    assert.deepEqual(await page.locator("#deploy-firmware option").evaluateAll(items => items.map(item => item.value)), ["old-image", "new-image"]);
    assert.equal(await page.locator("#deploy-firmware").inputValue(), "old-image");
    await page.locator("#deploy-cancel").click();

    // Device-card deployment must also refresh firmware, preserving the chosen board.
    await page.locator("#nav-devices").click();
    await page.locator('[data-device-deploy="new-board"]').waitFor();
    images = [firmware("newest-image")];
    await page.locator('[data-device-deploy="new-board"]').click();
    await page.locator("#deploy-overlay").waitFor({ state: "visible" });
    assert.equal(await page.locator("#deploy-device").inputValue(), "new-board");
    assert.deepEqual(await page.locator("#deploy-firmware option").evaluateAll(items => items.map(item => item.value)), ["newest-image"]);
    await page.locator("#deploy-cancel").click();

    // A failed refresh must never reopen a dialog populated from the old cache.
    failure = "/api/ota/firmware";
    await page.locator('[data-device-deploy="new-board"]').click();
    await page.locator("#toast").filter({ hasText: "Deployment list unavailable" }).waitFor();
    assert.equal(await page.locator("#deploy-overlay").isHidden(), true);

    // A selected image removed in another tab must not silently select a replacement.
    failure = "";
    await page.evaluate(async () => {
      showSection("ota-section", "ota");
      await loadOtaDashboard();
    });
    await page.locator('[data-firmware-deploy="newest-image"]').waitFor();
    images = [firmware("replacement-image")];
    await page.locator('[data-firmware-deploy="newest-image"]').click();
    await page.locator("#toast").filter({ hasText: "This firmware is no longer ready" }).waitFor();
    assert.equal(await page.locator("#deploy-overlay").isHidden(), true);

    // Removed boards clear stale choices and show the registration instruction.
    devices = [];
    await page.locator('[data-firmware-deploy="replacement-image"]').click();
    await page.locator("#toast").filter({ hasText: "Register an ESP32 device" }).waitFor();
    assert.equal(await page.locator("#deploy-overlay").isHidden(), true);
    assert.deepEqual(mutations, []);
    assert.deepEqual(errors, []);
    console.log("PASS: deployment refreshes devices and ready firmware, preserves selection, and reports refresh failures without using stale data");
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
