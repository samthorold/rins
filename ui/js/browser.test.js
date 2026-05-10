// Headless-browser smoke test for the UI shell.
//
// Drives the page in real Chromium to verify the parts that unit tests
// can't reach: file picker mounts the panel grid, drop-zone responds to
// real DataTransfer, year-picker broadcasts a cursor into every panel
// slot, and the panel grid is hidden until a file is loaded.
//
// Skipped automatically if Playwright's chromium is not available, so
// this file is safe to run as part of `npm test` without forcing the
// browser dependency on every contributor.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { join, resolve, extname, normalize, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const UI_ROOT = resolve(HERE, "..");
const REPO_ROOT = resolve(UI_ROOT, "..");

let chromium;
try {
  ({ chromium } = await import("playwright"));
} catch {
  test("browser tests skipped (playwright not installed)", { skip: true }, () => {});
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".ndjson": "application/x-ndjson; charset=utf-8",
};

async function startServer() {
  const server = createServer(async (req, res) => {
    const clean = normalize(decodeURIComponent((req.url || "/").split("?")[0]));
    const candidates = [
      join(UI_ROOT, clean === "/" ? "index.html" : clean),
      join(REPO_ROOT, clean),
    ];
    for (const p of candidates) {
      if (!p.startsWith(UI_ROOT) && !p.startsWith(REPO_ROOT)) continue;
      try {
        await stat(p);
        const data = await readFile(p);
        res.writeHead(200, { "content-type": MIME[extname(p)] || "application/octet-stream" });
        res.end(data);
        return;
      } catch {}
    }
    res.writeHead(404);
    res.end("not found");
  });
  await new Promise((r) => server.listen(0, r));
  const port = server.address().port;
  return { url: `http://127.0.0.1:${port}`, close: () => new Promise((r) => server.close(r)) };
}

const FIXTURE = [
  { day: 0, event: { SimulationStart: { year_start: 1, warmup_years: 0, analysis_years: 2 } } },
  { day: 0, event: { InsurerEntered: { insurer_id: 1, initial_capital: 1_000_000_000 } } },
  { day: 0, event: { YearStart: { year: 1 } } },
  { day: 5, event: { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[1, 1.0]], premium: 10, sum_insured: 100 } } },
  { day: 359, event: { YearEnd: { year: 1 } } },
  { day: 360, event: { YearStart: { year: 2 } } },
  { day: 719, event: { YearEnd: { year: 2 } } },
].map((e) => JSON.stringify(e)).join("\n");

if (chromium) {
  test("page loads without console errors", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", (err) => errors.push(err.message));
    page.on("console", (msg) => { if (msg.type() === "error") errors.push(msg.text()); });
    await page.goto(server.url);
    await page.waitForSelector("#drop-zone");
    assert.deepEqual(errors, []);
  });

  test("panel grid is hidden until a file is loaded", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    await page.goto(server.url);
    assert.equal(await page.locator("#panel-grid").isVisible(), false);
    assert.equal(await page.locator("#drop-zone").isVisible(), true);
    assert.equal(await page.locator("#year-picker").isVisible(), false);
  });

  test("loading a file via the picker reveals 8 panel slots and the year picker", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    await page.goto(server.url);

    await page.setInputFiles("#file-input", {
      name: "fixture.ndjson",
      mimeType: "application/x-ndjson",
      buffer: Buffer.from(FIXTURE),
    });

    await page.waitForSelector("#panel-grid:not(.hidden)");
    assert.equal(await page.locator("#drop-zone").isVisible(), false);
    assert.equal(await page.locator("#year-picker").isVisible(), true);
    const slotCount = await page.locator("#panel-grid .slot").count();
    assert.equal(slotCount, 8);

    const status = await page.locator("#status").textContent();
    assert.match(status, /fixture\.ndjson/);
    assert.match(status, /events/);
    assert.match(status, /\d+ years/);
  });

  test("panel 1 renders an SVG chart with traces after a file is loaded", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    await page.goto(server.url);
    await page.setInputFiles("#file-input", {
      name: "fixture.ndjson",
      mimeType: "application/x-ndjson",
      buffer: Buffer.from(FIXTURE),
    });
    await page.waitForSelector("#panel-1 svg");
    assert.equal(await page.locator("#panel-1 svg").count(), 1);
    // Capital area trace is always present once we have any rows.
    assert.equal(await page.locator("#panel-1 svg path.trace-capital").count(), 1);
  });

  test("panel 2 renders both risk-pooling sub-panels after a file is loaded", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    await page.goto(server.url);
    await page.setInputFiles("#file-input", {
      name: "fixture.ndjson",
      mimeType: "application/x-ndjson",
      buffer: Buffer.from(FIXTURE),
    });
    await page.waitForSelector("#panel-2 svg");
    assert.equal(await page.locator("#panel-2 svg g.subpanel-attr").count(), 1);
    assert.equal(await page.locator("#panel-2 svg g.subpanel-cat").count(), 1);
  });

  test("year-picker broadcasts a cursor into every panel slot", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    await page.goto(server.url);
    await page.setInputFiles("#file-input", {
      name: "fixture.ndjson",
      mimeType: "application/x-ndjson",
      buffer: Buffer.from(FIXTURE),
    });
    await page.waitForSelector("#panel-grid:not(.hidden)");

    // No cursor yet.
    assert.equal(await page.locator(".slot.has-cursor").count(), 0);

    // Set the year — the input fires `change` on blur/Enter.
    await page.locator("#year-input").fill("2");
    await page.locator("#year-input").press("Enter");

    await page.waitForFunction(
      () => document.querySelectorAll(".slot.has-cursor").length === 8,
    );
    const cursors = await page.locator(".slot.has-cursor").evaluateAll(
      (els) => els.map((e) => e.getAttribute("data-cursor")),
    );
    assert.equal(cursors.length, 8);
    for (const c of cursors) assert.match(c, /year 2/);
    assert.match(await page.locator("#selected-year").textContent(), /year 2/);

    // Clearing the year removes every cursor.
    await page.locator("#year-clear").click();
    await page.waitForFunction(
      () => document.querySelectorAll(".slot.has-cursor").length === 0,
    );
    assert.equal((await page.locator("#selected-year").textContent()).trim(), "");
  });

  test("panel 7 renders a sortable table and broadcasts on row click", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    await page.goto(server.url);
    await page.setInputFiles("#file-input", {
      name: "fixture.ndjson",
      mimeType: "application/x-ndjson",
      buffer: Buffer.from(FIXTURE),
    });
    await page.waitForSelector("#panel-7 table.p7-table");
    const dataRows = await page.locator("#panel-7 tbody tr[data-year]").count();
    assert.ok(dataRows >= 1);

    // Click a row → year-selected broadcast hits all 8 slots.
    await page.locator("#panel-7 tbody tr[data-year]").first().click();
    await page.waitForFunction(
      () => document.querySelectorAll(".slot.has-cursor").length === 8,
    );
    // Clicked row gains the selected class.
    assert.equal(
      await page.locator("#panel-7 tbody tr.p7-selected").count(),
      1,
    );
  });

  test("drop-zone accepts a real DataTransfer drop", async (t) => {
    const server = await startServer();
    const browser = await chromium.launch();
    t.after(async () => { await browser.close(); await server.close(); });
    const page = await browser.newPage();
    await page.goto(server.url);

    // Build a real DataTransfer with a File and dispatch drop on the zone.
    const dataTransfer = await page.evaluateHandle((text) => {
      const dt = new DataTransfer();
      dt.items.add(new File([text], "fixture.ndjson", { type: "application/x-ndjson" }));
      return dt;
    }, FIXTURE);
    await page.dispatchEvent("#drop-zone", "drop", { dataTransfer });

    await page.waitForSelector("#panel-grid:not(.hidden)");
    assert.match(await page.locator("#status").textContent(), /fixture\.ndjson/);
  });
}
