import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import { prepPanel7Data, renderPanel7, sortRows, formatInsurerCell } from "./panel7.js";

function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));
  push(0, { SimulationStart: { year_start: 1, warmup_years: 1, analysis_years: 3 } });
  push(0, { InsurerEntered: { insurer_id: 1, initial_capital: 1000, cr_sensitivity: 1.0, capacity_sensitivity: 0.2, market_weight_floor: 0.3 } });
  push(0, { InsurerEntered: { insurer_id: 2, initial_capital: 1000, cr_sensitivity: 2.0, capacity_sensitivity: 0.4, market_weight_floor: 0.3 } });

  // Year 1 (warmup).
  push(0, { YearStart: { year: 1 } });
  push(1, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 200, territory: "US-NE", perils_covered: ["Attritional"] } } });
  push(5, { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[1, 1.0]], premium: 10, sum_insured: 200 } });
  push(50, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 8, peril: "Attritional", remaining_capital: 992 } });
  push(60, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 8 } });
  push(140, { SubmissionDropped: { submission_id: 9, insured_id: 2 } });
  push(359, { YearEnd: { year: 1 } });

  // Year 2 (analysis).
  push(360, { YearStart: { year: 2 } });
  push(361, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 200, territory: "US-NE", perils_covered: ["Attritional"] } } });
  push(365, { PolicyBound: { policy_id: 1, submission_id: 1, insured_id: 1, panel: [[1, 1.0]], premium: 20, sum_insured: 200 } });
  push(400, { LossEvent: { event_id: 1, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0.05 } });
  push(401, { AssetDamage: { insured_id: 1, peril: "WindstormAtlantic", ground_up_loss: 4 } });
  push(450, { ClaimSettled: { policy_id: 1, insurer_id: 1, amount: 4, peril: "Attritional", remaining_capital: 988 } });
  push(719, { YearEnd: { year: 2 } });

  // Year 3 (analysis): insolvency, more claims.
  push(720, { YearStart: { year: 3 } });
  push(721, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 300, territory: "US-NE", perils_covered: ["Attritional"] } } });
  push(725, { PolicyBound: { policy_id: 2, submission_id: 2, insured_id: 1, panel: [[1, 1.0]], premium: 30, sum_insured: 300 } });
  push(820, { ClaimSettled: { policy_id: 2, insurer_id: 1, amount: 15, peril: "Attritional", remaining_capital: 973 } });
  push(900, { InsurerInsolvent: { insurer_id: 2 } });
  push(1079, { YearEnd: { year: 3 } });

  return lines.join("\n");
}

test("prepPanel7Data skips warmup years by default", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel7Data(db, { expenseRatio: 0.3 });
  assert.deepEqual(out.rows.map((r) => r.year), [2, 3]);
  assert.equal(out.warmupYears, 1);
});

test("prepPanel7Data includes warmup when requested", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel7Data(db, { expenseRatio: 0.3, includeWarmup: true });
  assert.deepEqual(out.rows.map((r) => r.year), [1, 2, 3]);
});

test("prepPanel7Data computes ratios and CR EWMA", () => {
  const db = parseNDJSONText(buildFixture());
  // EWMA seeds from y2 (warmup excluded).  y2 CR=4/20+0.3=0.5; y3 CR=15/30+0.3=0.8.
  // EWMA(α=1/3): y2 = 0.5; y3 = (1/3)*0.8 + (2/3)*0.5 = 0.6.
  const out = prepPanel7Data(db, { expenseRatio: 0.3, ewmaAlpha: 1 / 3 });
  const [y2, y3] = out.rows;
  assert.ok(Math.abs(y2.loss_ratio - 0.2) < 1e-9);
  assert.ok(Math.abs(y2.combined_ratio - 0.5) < 1e-9);
  assert.ok(Math.abs(y2.cr_ewma - 0.5) < 1e-9);
  assert.ok(Math.abs(y3.combined_ratio - 0.8) < 1e-9);
  assert.ok(Math.abs(y3.cr_ewma - 0.6) < 1e-9);
  assert.ok(Math.abs(y2.rate_on_line - 0.10) < 1e-9);
});

test("prepPanel7Data computes ap_tp from cr_ewma", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel7Data(db, { expenseRatio: 0.3, ewmaAlpha: 1 / 3 });
  const [y2, y3] = out.rows;
  // ap_tp = 1 + clamp(cr_ewma - 1, -0.10, 0.80)
  // y2: 1 + clamp(-0.5, ...) = 1 + -0.10 = 0.90
  assert.ok(Math.abs(y2.ap_tp - 0.90) < 1e-9);
  // y3: 1 + clamp(-0.4, ...) = 1 + -0.10 = 0.90
  assert.ok(Math.abs(y3.ap_tp - 0.90) < 1e-9);
});

test("prepPanel7Data exposes insurer count and delta", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel7Data(db, { expenseRatio: 0.3 });
  const [y2, y3] = out.rows;
  assert.equal(y2.insurer_count, 2);
  assert.equal(y2.entrants, 0);
  assert.equal(y2.insolvencies, 0);
  assert.equal(y3.insurer_count, 1);
  assert.equal(y3.insolvencies, 1);
});

test("prepPanel7Data carries cat_gul_pct and totals", () => {
  const db = parseNDJSONText(buildFixture());
  const out = prepPanel7Data(db, { expenseRatio: 0.3 });
  const [y2] = out.rows;
  // y2 has 4 cat GUL, 0 attr GUL → cat_gul_pct = 100.
  assert.equal(y2.gul, 4);
  assert.equal(y2.cat_gul_pct, 100);
  assert.equal(y2.cat_event_count, 1);
  assert.equal(y2.total_assets, 200);
  assert.equal(y2.coverage, 200);
  assert.equal(y2.claims, 4);
  assert.equal(y2.dropped, 0);
});

test("formatInsurerCell renders +/- delta", () => {
  assert.equal(formatInsurerCell({ insurer_count: 8, entrants: 0, insolvencies: 0 }), "8");
  assert.equal(formatInsurerCell({ insurer_count: 9, entrants: 1, insolvencies: 0 }), "9 +1");
  assert.equal(formatInsurerCell({ insurer_count: 7, entrants: 0, insolvencies: 1 }), "7 -1");
  assert.equal(formatInsurerCell({ insurer_count: 8, entrants: 1, insolvencies: 1 }), "8 +1-1");
});

test("sortRows sorts by numeric column ascending and descending", () => {
  const rows = [
    { year: 1, claims: 30 },
    { year: 2, claims: 10 },
    { year: 3, claims: 20 },
  ];
  const asc = sortRows(rows, "claims", "asc");
  assert.deepEqual(asc.map((r) => r.year), [2, 3, 1]);
  const desc = sortRows(rows, "claims", "desc");
  assert.deepEqual(desc.map((r) => r.year), [1, 3, 2]);
});

test("sortRows treats null/undefined as worst per direction", () => {
  const rows = [
    { year: 1, claims: 5 },
    { year: 2, claims: null },
    { year: 3, claims: 10 },
  ];
  // Ascending: null sorts last.
  assert.deepEqual(sortRows(rows, "claims", "asc").map((r) => r.year), [1, 3, 2]);
  // Descending: null sorts last too (always least useful).
  assert.deepEqual(sortRows(rows, "claims", "desc").map((r) => r.year), [3, 1, 2]);
});

test("renderPanel7 returns a node with one row per year", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel7Data(db, { expenseRatio: 0.3 });
  const node = renderPanel7(data, { asString: true });
  // 2 analysis years → 2 data rows.
  const rowMatches = node.match(/<tr data-year=/g) ?? [];
  assert.equal(rowMatches.length, 2);
  // Header row exists.
  assert.ok(node.includes("<thead"));
  // Year column header.
  assert.ok(node.includes(">Year<"));
});
