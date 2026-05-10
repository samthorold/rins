import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import { prepPanel1Data, renderPanel1, ewmaSeries } from "./panel1.js";

// Build a synthetic stream we can hand-compute against.
function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));
  push(0, { SimulationStart: { year_start: 1, warmup_years: 1, analysis_years: 3 } });
  push(0, { InsurerEntered: { insurer_id: 1, initial_capital: 1000 } });
  push(0, { InsurerEntered: { insurer_id: 2, initial_capital: 1000 } });

  // Year 1 (warmup): premium=10 si=200 claims=8 → LR=0.8, CR=0.8+0.3=1.1
  push(0, { YearStart: { year: 1 } });
  push(5, { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[1, 1.0]], premium: 10, sum_insured: 200 } });
  push(50, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 8, peril: "Attritional", remaining_capital: 992 } });
  push(359, { YearEnd: { year: 1 } });

  // Year 2 (analysis): premium=20 si=200 claims=4 → RoL=10%, LR=0.2, CR=0.5
  push(360, { YearStart: { year: 2 } });
  push(365, { PolicyBound: { policy_id: 1, submission_id: 1, insured_id: 1, panel: [[1, 1.0]], premium: 20, sum_insured: 200 } });
  push(400, { LossEvent: { event_id: 1, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0.1 } });
  push(401, { LossEvent: { event_id: 2, peril: "WindstormAtlantic", territory: "US-Gulf", damage_fraction: 0.1 } });
  push(450, { ClaimSettled: { policy_id: 1, insurer_id: 1, amount: 4, peril: "Attritional", remaining_capital: 988 } });
  push(500, { InsurerEntered: { insurer_id: 3, initial_capital: 500 } });
  push(719, { YearEnd: { year: 2 } });

  // Year 3 (analysis): premium=30 si=300 claims=15 → RoL=10%, LR=0.5, CR=0.8
  push(720, { YearStart: { year: 3 } });
  push(725, { PolicyBound: { policy_id: 2, submission_id: 2, insured_id: 1, panel: [[1, 1.0]], premium: 30, sum_insured: 300 } });
  push(820, { ClaimSettled: { policy_id: 2, insurer_id: 1, amount: 15, peril: "Attritional", remaining_capital: 973 } });
  push(900, { InsurerInsolvent: { insurer_id: 2 } });
  push(1079, { YearEnd: { year: 3 } });

  // Year 4 (analysis, empty): no policies/claims; capital carries forward
  push(1080, { YearStart: { year: 4 } });
  push(1439, { YearEnd: { year: 4 } });

  return lines.join("\n");
}

test("ewmaSeries(alpha=1/3) initialises to first value and recurses", () => {
  const series = ewmaSeries([1, 2, 3], 1 / 3);
  assert.equal(series.length, 3);
  assert.equal(series[0], 1);
  // E1 = 1/3 * 2 + 2/3 * 1 = 4/3
  assert.ok(Math.abs(series[1] - 4 / 3) < 1e-9);
  // E2 = 1/3 * 3 + 2/3 * 4/3 = 1 + 8/9 = 17/9
  assert.ok(Math.abs(series[2] - 17 / 9) < 1e-9);
});

test("ewmaSeries returns empty for empty input", () => {
  assert.deepEqual(ewmaSeries([], 1 / 3), []);
});

test("prepPanel1Data skips warmup years by default", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel1Data(db, { expenseRatio: 0.3 });
  // Warmup = 1, so years 2,3,4 only.
  assert.deepEqual(out.rows.map((r) => r.year), [2, 3, 4]);
  assert.equal(out.warmupYears, 1);
});

test("prepPanel1Data computes rate_on_line and combined_ratio", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel1Data(db, { expenseRatio: 0.3 });
  const [y2, y3, y4] = out.rows;

  // y2: 20/200 = 0.10; LR=4/20=0.2; CR=0.5
  assert.ok(Math.abs(y2.rate_on_line - 0.10) < 1e-9);
  assert.ok(Math.abs(y2.combined_ratio - 0.5) < 1e-9);

  // y3: 30/300 = 0.10; LR=15/30=0.5; CR=0.8
  assert.ok(Math.abs(y3.rate_on_line - 0.10) < 1e-9);
  assert.ok(Math.abs(y3.combined_ratio - 0.8) < 1e-9);

  // y4: no premium → rate_on_line null (avoid div by zero); CR also null
  assert.equal(y4.rate_on_line, null);
  assert.equal(y4.combined_ratio, null);
});

test("prepPanel1Data computes CR EWMA over the analysis window", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel1Data(db, { expenseRatio: 0.3, ewmaAlpha: 1 / 3 });
  const [y2, y3, y4] = out.rows;
  // Values fed to EWMA skip nulls; resulting series is aligned by index.
  // y2 CR=0.5; y3 CR = 1/3 * 0.8 + 2/3 * 0.5 = 0.6
  assert.ok(Math.abs(y2.cr_ewma - 0.5) < 1e-9);
  assert.ok(Math.abs(y3.cr_ewma - 0.6) < 1e-9);
  // y4 has no CR — EWMA carries previous value.
  assert.ok(Math.abs(y4.cr_ewma - 0.6) < 1e-9);
});

test("prepPanel1Data exposes capital and annotations", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel1Data(db, { expenseRatio: 0.3 });
  const [y2, y3] = out.rows;
  assert.equal(y2.cat_event_count, 2);
  assert.equal(y2.entrants, 1);
  assert.equal(y3.insolvencies, 1);
  assert.ok(y2.total_capital > 0);
});

test("prepPanel1Data supports includeWarmup option", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel1Data(db, { expenseRatio: 0.3, includeWarmup: true });
  assert.deepEqual(out.rows.map((r) => r.year), [1, 2, 3, 4]);
});

test("renderPanel1 returns an SVG element with chart structure", () => {
  // Use a tiny synthetic data object — no DOM lib needed: render returns a string
  // when no document is provided, so the unit test stays headless.
  const data = {
    rows: [
      { year: 2, rate_on_line: 0.05, combined_ratio: 0.7, cr_ewma: 0.7, total_capital: 1000, cat_event_count: 0, entrants: 0, insolvencies: 0 },
      { year: 3, rate_on_line: 0.10, combined_ratio: 0.9, cr_ewma: 0.8, total_capital: 950, cat_event_count: 2, entrants: 1, insolvencies: 0 },
      { year: 4, rate_on_line: 0.08, combined_ratio: 1.1, cr_ewma: 0.9, total_capital: 900, cat_event_count: 0, entrants: 0, insolvencies: 1 },
    ],
    warmupYears: 1,
    ewmaAlpha: 1 / 3,
    expenseRatio: 0.3,
  };
  const svg = renderPanel1(data, { asString: true });
  assert.equal(typeof svg, "string");
  assert.match(svg, /<svg/);
  // Has all four traces (class hooks for CSS / testability).
  assert.match(svg, /class="trace-rol"/);
  assert.match(svg, /class="trace-cr"/);
  assert.match(svg, /class="trace-cr-ewma"/);
  assert.match(svg, /class="trace-capital"/);
  // Annotations: cat band (year 3, count >= 2), entrant marker (year 3), insolvency marker (year 4).
  assert.match(svg, /class="cat-band"/);
  assert.match(svg, /class="entrant-marker"/);
  assert.match(svg, /class="insolvent-marker"/);
  // CR=100% reference line.
  assert.match(svg, /class="cr-100-ref"/);
});

test("renderPanel1 with empty rows returns a placeholder SVG without throwing", () => {
  const svg = renderPanel1({ rows: [], warmupYears: 0, ewmaAlpha: 1 / 3, expenseRatio: 0 }, { asString: true });
  assert.match(svg, /<svg/);
  assert.match(svg, /no data/i);
});
