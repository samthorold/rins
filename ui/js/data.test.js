import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import { parseNDJSONText } from "./data.js";

const HERE = dirname(fileURLToPath(import.meta.url));

// ---------- Fixture builder ----------
// Build a synthetic event stream to test the data layer in isolation.
function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));

  // Sim start: 2 warmup years + 3 analysis years, total 5 years.
  push(0, { SimulationStart: { year_start: 1, warmup_years: 2, analysis_years: 3 } });
  push(0, { InsurerEntered: { insurer_id: 1, initial_capital: 1_000_000_000, cr_sensitivity: 1.0, capacity_sensitivity: 0.1, market_weight_floor: 0.3 } });
  push(0, { InsurerEntered: { insurer_id: 2, initial_capital: 1_000_000_000, cr_sensitivity: 1.0, capacity_sensitivity: 0.1, market_weight_floor: 0.3 } });

  // ---- Year 1 (days 0-359) ----
  push(0, { YearStart: { year: 1 } });
  push(1, { LeadQuoteRequested: { submission_id: 0, insured_id: 1, insurer_id: 1, risk: { sum_insured: 100, territory: "US-NE", perils_covered: ["WindstormAtlantic", "Attritional"] } } });
  push(1, { LeadQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 1, atp: 5, premium: 10, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(2, { QuoteAccepted: { submission_id: 0, insured_id: 1, leader_id: 1, panel: [[1, 0.5], [2, 0.5]], premium: 10 } });
  push(3, { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[1, 0.5], [2, 0.5]], premium: 10, sum_insured: 100 } });
  push(50, { LossEvent: { event_id: 0, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0.10 } });
  push(50, { AssetDamage: { insured_id: 1, peril: "WindstormAtlantic", ground_up_loss: 10 } });
  push(50, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 5, peril: "WindstormAtlantic", remaining_capital: 999_999_995 } });
  push(50, { ClaimSettled: { policy_id: 0, insurer_id: 2, amount: 5, peril: "WindstormAtlantic", remaining_capital: 999_999_995 } });
  push(60, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 4 } });
  push(60, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 2, peril: "Attritional", remaining_capital: 999_999_993 } });
  push(60, { ClaimSettled: { policy_id: 0, insurer_id: 2, amount: 2, peril: "Attritional", remaining_capital: 999_999_993 } });
  push(140, { SubmissionDropped: { submission_id: 1, insured_id: 2 } });
  push(359, { YearEnd: { year: 1 } });

  // ---- Year 2 (days 360-719) ---- (still warmup)
  push(360, { YearStart: { year: 2 } });
  push(363, { PolicyBound: { policy_id: 1, submission_id: 2, insured_id: 1, panel: [[1, 1.0]], premium: 20, sum_insured: 200 } });
  push(719, { YearEnd: { year: 2 } });

  // ---- Year 3 (days 720-1079) ---- (analysis year)
  push(720, { YearStart: { year: 3 } });
  push(723, { PolicyBound: { policy_id: 2, submission_id: 3, insured_id: 1, panel: [[1, 1.0]], premium: 30, sum_insured: 300 } });
  push(723, { PolicyBound: { policy_id: 3, submission_id: 4, insured_id: 2, panel: [[2, 1.0]], premium: 30, sum_insured: 300 } });
  push(800, { LossEvent: { event_id: 1, peril: "WindstormAtlantic", territory: "US-Gulf", damage_fraction: 0.5 } });
  push(800, { LossEvent: { event_id: 2, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0.5 } });
  push(900, { InsurerInsolvent: { insurer_id: 2 } });
  push(1079, { YearEnd: { year: 3 } });

  return lines.join("\n");
}

// ---------- Parser tests ----------

test("parseNDJSONText returns a database object", () => {
  const db = parseNDJSONText(buildFixture());
  assert.equal(typeof db, "object");
  assert.equal(typeof db.getYearStats, "function");
  assert.equal(typeof db.getEventsByType, "function");
  assert.equal(typeof db.getEventsByInsurer, "function");
  assert.equal(typeof db.getWarmupYears, "function");
});

test("getWarmupYears reads from SimulationStart", () => {
  const db = parseNDJSONText(buildFixture());
  assert.equal(db.getWarmupYears(), 2);
});

test("ignores blank lines and tolerates trailing newline", () => {
  const text = buildFixture() + "\n\n";
  const db = parseNDJSONText(text);
  assert.equal(db.getWarmupYears(), 2);
});

test("getEventsByType returns events of one type", () => {
  const db = parseNDJSONText(buildFixture());
  const bound = db.getEventsByType("PolicyBound");
  assert.equal(bound.length, 4);
  assert.equal(bound[0].data.policy_id, 0);
  assert.equal(bound[0].day, 3);
  assert.equal(bound[0].year, 1);
  assert.equal(bound[0].type, "PolicyBound");
});

test("getEventsByType filtered by year", () => {
  const db = parseNDJSONText(buildFixture());
  const y1 = db.getEventsByType("PolicyBound", 1);
  assert.equal(y1.length, 1);
  assert.equal(y1[0].data.policy_id, 0);
  const y3 = db.getEventsByType("PolicyBound", 3);
  assert.equal(y3.length, 2);
});

test("getEventsByType returns empty array for unknown type", () => {
  const db = parseNDJSONText(buildFixture());
  assert.deepEqual(db.getEventsByType("NoSuchType"), []);
});

test("getEventsByInsurer returns events tagged with insurer_id", () => {
  const db = parseNDJSONText(buildFixture());
  const ins1 = db.getEventsByInsurer(1);
  // InsurerEntered, LeadQuoteRequested, LeadQuoteIssued, ClaimSettled (windstorm),
  // ClaimSettled (attritional), PolicyBound y2, PolicyBound y3.
  // Note: PolicyBound has no top-level insurer_id field but does have panel.
  // Spec: events tagged via inner `insurer_id` only.
  const types = ins1.map((e) => e.type).sort();
  assert.ok(types.includes("InsurerEntered"));
  assert.ok(types.includes("LeadQuoteIssued"));
  assert.ok(types.includes("ClaimSettled"));
  // Insurer 2's claim settle should not appear.
  for (const e of ins1) {
    if (e.type === "ClaimSettled") {
      assert.equal(e.data.insurer_id, 1);
    }
  }
});

test("getEventsByInsurer filters by year", () => {
  const db = parseNDJSONText(buildFixture());
  const y1 = db.getEventsByInsurer(1, 1);
  for (const e of y1) assert.equal(e.year, 1);
});

// ---------- YearStats tests ----------

test("getYearStats returns one entry per year", () => {
  const db = parseNDJSONText(buildFixture());
  const stats = db.getYearStats();
  assert.equal(stats.length, 3);
  assert.deepEqual(
    stats.map((s) => s.year),
    [1, 2, 3],
  );
});

test("YearStats aggregates premium and sum_insured per year", () => {
  const db = parseNDJSONText(buildFixture());
  const [y1, y2, y3] = db.getYearStats();
  assert.equal(y1.bound_premium, 10);
  assert.equal(y1.sum_insured, 100);
  assert.equal(y2.bound_premium, 20);
  assert.equal(y3.bound_premium, 60);
  assert.equal(y3.sum_insured, 600);
});

test("YearStats aggregates claims and GUL by peril", () => {
  const db = parseNDJSONText(buildFixture());
  const [y1] = db.getYearStats();
  assert.equal(y1.claims, 14); // 5+5+2+2
  assert.equal(y1.cat_gul, 10);
  assert.equal(y1.attr_gul, 4);
});

test("YearStats counts cat events, entrants, insolvencies, dropped", () => {
  const db = parseNDJSONText(buildFixture());
  const [y1, , y3] = db.getYearStats();
  assert.equal(y1.cat_event_count, 1);
  assert.equal(y1.insolvencies, 0);
  assert.equal(y1.dropped, 1);
  assert.equal(y3.cat_event_count, 2);
  assert.equal(y3.insolvencies, 1);
  assert.equal(y3.entrants, 0);
});

test("YearStats counts InsurerEntered as entrants in year 1", () => {
  const db = parseNDJSONText(buildFixture());
  const [y1] = db.getYearStats();
  assert.equal(y1.entrants, 2);
});

test("YearStats total_capital reflects last remaining_capital per insurer", () => {
  const db = parseNDJSONText(buildFixture());
  const [y1] = db.getYearStats();
  // Insurer 1 last claim: 999_999_993, insurer 2 last claim: 999_999_993, sum ~2e9.
  assert.equal(y1.total_capital, 999_999_993 + 999_999_993);
});

test("YearStats total_capital falls back to initial_capital when no claims", () => {
  const db = parseNDJSONText(buildFixture());
  const [, y2] = db.getYearStats();
  // No claims in year 2 → fall back to last known per insurer (year 1's last).
  assert.equal(y2.total_capital, 999_999_993 + 999_999_993);
});

test("YearStats gini = 0 when policies are evenly distributed", () => {
  const db = parseNDJSONText(buildFixture());
  const [, , y3] = db.getYearStats();
  // Year 3: 1 policy each for insurers 1 & 2 → gini = 0.
  assert.equal(y3.gini, 0);
});

test("YearStats gini > 0 with concentration", () => {
  const db = parseNDJSONText(buildFixture());
  const [, y2] = db.getYearStats();
  // Year 2: only insurer 1 bound a policy → max gini.
  assert.ok(y2.gini > 0);
});

// ---------- Real events.ndjson smoke test ----------

test("loads a real events.ndjson if present (smoke)", () => {
  const real = resolve(HERE, "../../events.ndjson");
  if (!existsSync(real)) return;
  const text = readFileSync(real, "utf8");
  // Only first ~50K lines to keep test fast.
  const slice = text.split("\n", 50_000).join("\n");
  const db = parseNDJSONText(slice);
  assert.ok(db.getWarmupYears() >= 0);
  assert.ok(db.getYearStats().length >= 1);
});
