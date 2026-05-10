import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import {
  prepPanel10Data,
  renderPanel10,
  PHENOMENA,
  detectCycle,
  detectCatCrisis,
  detectMarketEntry,
  detectInsolvencies,
} from "./panel10.js";

function buildStream(opts) {
  const {
    years,
    warmup = 0,
    insurers = [{ id: 1, capital: 1000 }],
    extras = [],
  } = opts;
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));
  push(0, { SimulationStart: { year_start: 1, warmup_years: warmup, analysis_years: years.length - warmup } });
  for (const i of insurers) push(0, { InsurerEntered: { insurer_id: i.id, initial_capital: i.capital } });
  let policyId = 0, subId = 0;
  for (let i = 0; i < years.length; i++) {
    const y = i + 1;
    const dayBase = i * 360;
    const yr = years[i];
    push(dayBase, { YearStart: { year: y } });
    if (yr.bound) {
      const { premium, sum_insured, claims } = yr.bound;
      push(dayBase + 5, { PolicyBound: { policy_id: policyId, submission_id: subId, insured_id: 1, panel: [[1, 1.0]], premium, sum_insured } });
      push(dayBase + 50, { ClaimSettled: { policy_id: policyId, insurer_id: 1, amount: claims, peril: "Attritional", remaining_capital: 1000 - claims } });
      policyId += 1;
      subId += 1;
    }
    if (yr.cats) {
      for (let c = 0; c < yr.cats; c++) {
        push(dayBase + 100 + c, { LossEvent: { event_id: 1000 + c, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0.1 } });
      }
    }
    if (yr.entrants) {
      for (let e = 0; e < yr.entrants; e++) {
        push(dayBase + 200 + e, { InsurerEntered: { insurer_id: 100 + e, initial_capital: 500 } });
      }
    }
    if (yr.insolvent) {
      push(dayBase + 250, { InsurerInsolvent: { insurer_id: yr.insolvent } });
    }
    push(dayBase + 359, { YearEnd: { year: y } });
  }
  for (const ex of extras) push(ex[0], ex[1]);
  return lines.join("\n");
}

function dbFromYears(years, opts = {}) {
  return parseNDJSONText(buildStream({ years, ...opts }));
}

// ── Detector unit tests ───────────────────────────────────────────────────

test("detectCycle — true when RoL range > 2pp", () => {
  const db = dbFromYears([
    { bound: { premium: 5, sum_insured: 100, claims: 1 } },  // RoL 5%
    { bound: { premium: 10, sum_insured: 100, claims: 1 } }, // RoL 10% → range 5pp
  ]);
  assert.equal(detectCycle(db), true);
});

test("detectCycle — false when RoL range ≤ 2pp", () => {
  const db = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 } }, // 10%
    { bound: { premium: 11, sum_insured: 100, claims: 1 } }, // 11% → 1pp
  ]);
  assert.equal(detectCycle(db), false);
});

test("detectInsolvencies — true when at least one InsurerInsolvent in analysis", () => {
  const db = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 }, insolvent: 1 },
  ]);
  assert.equal(detectInsolvencies(db), true);
});

test("detectInsolvencies — false when no InsurerInsolvent fired", () => {
  const db = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 } },
  ]);
  assert.equal(detectInsolvencies(db), false);
});

test("detectMarketEntry — true when post-warmup InsurerEntered fires", () => {
  const db = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 }, entrants: 1 },
  ]);
  assert.equal(detectMarketEntry(db), true);
});

test("detectMarketEntry — false when only initial InsurerEntered events", () => {
  const db = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 } },
  ]);
  assert.equal(detectMarketEntry(db), false);
});

test("detectCatCrisis — true when a year has cats ≥ 2 AND at least one insolvency", () => {
  const db = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 }, cats: 2, insolvent: 1 },
  ]);
  assert.equal(detectCatCrisis(db), true);
});

test("detectCatCrisis — false when cats present but no insolvency anywhere", () => {
  const db = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 }, cats: 2 },
  ]);
  assert.equal(detectCatCrisis(db), false);
});

// ── prepPanel10Data ───────────────────────────────────────────────────────

test("PHENOMENA exposes the four detection metrics", () => {
  const ids = PHENOMENA.map((p) => p.id);
  assert.ok(ids.includes("cycle"));
  assert.ok(ids.includes("market_entry"));
  assert.ok(ids.includes("insolvencies"));
  assert.ok(ids.includes("cat_crisis"));
});

test("prepPanel10Data — counts detection across runs", () => {
  const dbCycle = dbFromYears([
    { bound: { premium: 5,  sum_insured: 100, claims: 1 } },
    { bound: { premium: 10, sum_insured: 100, claims: 1 } },
  ]);
  const dbFlat = dbFromYears([
    { bound: { premium: 10, sum_insured: 100, claims: 1 } },
    { bound: { premium: 11, sum_insured: 100, claims: 1 } },
  ]);
  const data = prepPanel10Data([dbCycle, dbFlat, dbCycle]);
  assert.equal(data.runCount, 3);
  const cycle = data.phenomena.find((p) => p.id === "cycle");
  assert.equal(cycle.detected, 2);
  assert.equal(cycle.total, 3);
  assert.ok(Math.abs(cycle.ratio - 2 / 3) < 1e-9);
});

test("prepPanel10Data — empty inputs return zero-detection rows", () => {
  const data = prepPanel10Data([]);
  assert.equal(data.runCount, 0);
  for (const p of data.phenomena) {
    assert.equal(p.detected, 0);
    assert.equal(p.total, 0);
    assert.equal(p.ratio, 0);
  }
});

// ── renderPanel10 ─────────────────────────────────────────────────────────

test("renderPanel10 — emits one row per phenomenon with detection ratio", () => {
  const dbCycle = dbFromYears([
    { bound: { premium: 5,  sum_insured: 100, claims: 1 } },
    { bound: { premium: 10, sum_insured: 100, claims: 1 } },
  ]);
  const data = prepPanel10Data([dbCycle, dbCycle]);
  const html = renderPanel10(data, { asString: true });
  assert.ok(html.includes("p10-row"));
  // Every phenomenon shows up as a row.
  for (const ph of PHENOMENA) {
    assert.ok(html.includes(ph.label), `should mention ${ph.label}`);
  }
  // Detection of cycle should show 2/2.
  assert.ok(html.includes("2 / 2") || html.includes("2/2"));
});

test("renderPanel10 — empty data renders without throwing", () => {
  const html = renderPanel10(prepPanel10Data([]), { asString: true });
  assert.ok(typeof html === "string");
  assert.ok(html.length > 0);
});
