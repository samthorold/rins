import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import {
  prepPanel9Data,
  renderPanel9,
  percentile,
  METRICS as PANEL9_METRICS,
} from "./panel9.js";

// Build a stream with a controllable per-year (premium, sum_insured, claims, capital).
// Each year has one PolicyBound + one ClaimSettled + a YearEnd.
function buildStream({ years, expenseRatio = 0.3, warmup = 0 }) {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));
  push(0, { SimulationStart: { year_start: 1, warmup_years: warmup, analysis_years: years.length - warmup } });
  push(0, { InsurerEntered: { insurer_id: 1, initial_capital: 1000 } });
  let policyId = 0;
  let subId = 0;
  for (let i = 0; i < years.length; i++) {
    const y = i + 1;
    const dayBase = i * 360;
    const { premium, sum_insured, claims, capital } = years[i];
    push(dayBase, { YearStart: { year: y } });
    push(dayBase + 5, { PolicyBound: { policy_id: policyId, submission_id: subId, insured_id: 1, panel: [[1, 1.0]], premium, sum_insured } });
    push(dayBase + 50, { ClaimSettled: { policy_id: policyId, insurer_id: 1, amount: claims, peril: "Attritional", remaining_capital: capital } });
    push(dayBase + 359, { YearEnd: { year: y } });
    policyId += 1;
    subId += 1;
  }
  return lines.join("\n");
}

function dbFromYears(years) {
  return parseNDJSONText(buildStream({ years }));
}

// ── percentile helper ─────────────────────────────────────────────────────

test("percentile — returns null for empty input", () => {
  assert.equal(percentile([], 50), null);
});

test("percentile — single value returns that value at any p", () => {
  assert.equal(percentile([42], 5), 42);
  assert.equal(percentile([42], 50), 42);
  assert.equal(percentile([42], 95), 42);
});

test("percentile — known case (linear interp)", () => {
  // values = [1,2,3,4,5], p50 → 3, p25 → 2, p75 → 4
  assert.equal(percentile([1, 2, 3, 4, 5], 50), 3);
  assert.equal(percentile([1, 2, 3, 4, 5], 25), 2);
  assert.equal(percentile([1, 2, 3, 4, 5], 75), 4);
});

test("percentile — handles unsorted input", () => {
  assert.equal(percentile([5, 1, 3, 2, 4], 50), 3);
});

test("percentile — ignores nulls/NaNs", () => {
  assert.equal(percentile([1, null, 2, undefined, 3, NaN], 50), 2);
});

// ── METRICS ───────────────────────────────────────────────────────────────

test("METRICS exposes the four canonical fan series", () => {
  const keys = PANEL9_METRICS.map((m) => m.key);
  assert.deepEqual(keys.sort(), ["combined_ratio", "loss_ratio", "rate_on_line", "total_capital"].sort());
});

// ── prepPanel9Data ────────────────────────────────────────────────────────

test("prepPanel9Data — returns one row per year per metric", () => {
  const dbA = dbFromYears([
    { premium: 10, sum_insured: 100, claims: 5, capital: 995 },
    { premium: 20, sum_insured: 100, claims: 5, capital: 990 },
  ]);
  const dbB = dbFromYears([
    { premium: 12, sum_insured: 100, claims: 6, capital: 994 },
    { premium: 22, sum_insured: 100, claims: 4, capital: 990 },
  ]);
  const data = prepPanel9Data([dbA, dbB]);
  assert.equal(data.runCount, 2);
  assert.equal(data.metrics.length, 4);
  for (const m of data.metrics) {
    assert.equal(m.rows.length, 2, `metric ${m.key} should have 2 year rows`);
    for (const r of m.rows) {
      assert.ok(typeof r.year === "number");
      assert.ok(typeof r.p50 === "number");
      assert.ok(r.p5 <= r.p25);
      assert.ok(r.p25 <= r.p50);
      assert.ok(r.p50 <= r.p75);
      assert.ok(r.p75 <= r.p95);
    }
  }
});

test("prepPanel9Data — rate_on_line p50 matches median of dbs", () => {
  const dbA = dbFromYears([{ premium: 10, sum_insured: 100, claims: 0, capital: 1000 }]);
  const dbB = dbFromYears([{ premium: 20, sum_insured: 100, claims: 0, capital: 1000 }]);
  const dbC = dbFromYears([{ premium: 30, sum_insured: 100, claims: 0, capital: 1000 }]);
  const data = prepPanel9Data([dbA, dbB, dbC]);
  const rol = data.metrics.find((m) => m.key === "rate_on_line");
  assert.ok(rol);
  // RoL values: 0.10, 0.20, 0.30 → p50 = 0.20
  assert.equal(rol.rows[0].p50, 0.2);
  assert.equal(rol.rows[0].p5, 0.10);
  assert.equal(rol.rows[0].p95, 0.30);
});

test("prepPanel9Data — combined_ratio applies expense_ratio offset", () => {
  const dbA = dbFromYears([{ premium: 100, sum_insured: 1000, claims: 50, capital: 950 }]);
  const data = prepPanel9Data([dbA], { expenseRatio: 0.3 });
  const cr = data.metrics.find((m) => m.key === "combined_ratio");
  // LR=0.5, CR = 0.5 + 0.3 = 0.8
  assert.equal(cr.rows[0].p50, 0.8);
});

test("prepPanel9Data — excludes warmup years by default", () => {
  // Build a stream with 1 warmup + 2 analysis years
  const stream = buildStream({
    years: [
      { premium: 10, sum_insured: 100, claims: 0, capital: 1000 },
      { premium: 20, sum_insured: 100, claims: 0, capital: 1000 },
      { premium: 30, sum_insured: 100, claims: 0, capital: 1000 },
    ],
    warmup: 1,
  });
  const db = parseNDJSONText(stream);
  const data = prepPanel9Data([db]);
  const rol = data.metrics.find((m) => m.key === "rate_on_line");
  // Years 2 and 3 only
  assert.deepEqual(rol.rows.map((r) => r.year), [2, 3]);
});

test("prepPanel9Data — empty database list returns empty metrics rows", () => {
  const data = prepPanel9Data([]);
  assert.equal(data.runCount, 0);
  for (const m of data.metrics) assert.equal(m.rows.length, 0);
});

test("prepPanel9Data — handles years that vary across runs (union)", () => {
  // Run A has 2 years, run B has 3 years.
  const dbA = dbFromYears([
    { premium: 10, sum_insured: 100, claims: 0, capital: 1000 },
    { premium: 20, sum_insured: 100, claims: 0, capital: 1000 },
  ]);
  const dbB = dbFromYears([
    { premium: 11, sum_insured: 100, claims: 0, capital: 1000 },
    { premium: 21, sum_insured: 100, claims: 0, capital: 1000 },
    { premium: 31, sum_insured: 100, claims: 0, capital: 1000 },
  ]);
  const data = prepPanel9Data([dbA, dbB]);
  const rol = data.metrics.find((m) => m.key === "rate_on_line");
  // Year 3 is only in B; p50 should still equal that single value.
  const yr3 = rol.rows.find((r) => r.year === 3);
  assert.ok(yr3);
  assert.equal(yr3.p50, 0.31);
});

// ── renderPanel9 ──────────────────────────────────────────────────────────

test("renderPanel9 — produces an svg with one band group per metric", () => {
  const dbA = dbFromYears([
    { premium: 10, sum_insured: 100, claims: 5, capital: 1000 },
    { premium: 20, sum_insured: 100, claims: 5, capital: 1000 },
  ]);
  const dbB = dbFromYears([
    { premium: 12, sum_insured: 100, claims: 6, capital: 1000 },
    { premium: 22, sum_insured: 100, claims: 4, capital: 1000 },
  ]);
  const data = prepPanel9Data([dbA, dbB]);
  const html = renderPanel9(data, { asString: true });
  assert.ok(html.includes("<svg"));
  // Each metric should have a p5-p95 band class.
  const bandCount = (html.match(/p9-band-outer/g) ?? []).length;
  assert.equal(bandCount, 4);
  // Should mention the run count somewhere.
  assert.ok(html.includes("2 runs") || html.includes("n=2"));
});

test("renderPanel9 — empty data renders without throwing", () => {
  const data = prepPanel9Data([]);
  const html = renderPanel9(data, { asString: true });
  assert.ok(html.includes("<svg") || html.includes("no data"));
});
