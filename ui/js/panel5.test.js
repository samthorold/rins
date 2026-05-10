import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import { prepPanel5Data, renderPanel5 } from "./panel5.js";

// Fixture: 1 warmup + 3 analysis years; 2 incumbents.
// Cat policies (WindstormAtlantic) accumulate cat_aggregate; PolicyExpired
// releases it. Territory exposure differs between the two insurers so that
// sub-panel B (territory split) has visible signal.
function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));

  push(0, { SimulationStart: { year_start: 1, warmup_years: 1, analysis_years: 3 } });
  for (const id of [1, 2]) {
    push(0, { InsurerEntered: { insurer_id: id, initial_capital: 1000 } });
  }

  // CoverageRequested gives us territory + perils for each insured. The
  // panel reads back from CR (most recent per insured) to classify binds.
  // Insured 1: cat-covered, US-NE
  // Insured 2: cat-covered, US-SE
  // Insured 3: attritional only, US-Gulf
  push(0, { YearStart: { year: 1 } });
  push(0, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: ["WindstormAtlantic", "Attritional"] } } });
  push(0, { CoverageRequested: { insured_id: 2, risk: { sum_insured: 1000, territory: "US-SE", perils_covered: ["WindstormAtlantic", "Attritional"] } } });
  push(0, { CoverageRequested: { insured_id: 3, risk: { sum_insured: 1000, territory: "US-Gulf", perils_covered: ["Attritional"] } } });

  // Y1 binds (warmup):
  // policy 1: insured 1 (US-NE, cat) bound to insurer 1 only (line 1.0).
  push(10, { PolicyBound: { policy_id: 1, submission_id: 1, insured_id: 1, panel: [[1, 1.0]], premium: 100, sum_insured: 1000 } });
  // policy 2: insured 2 (US-SE, cat) split 1+2 each 0.5.
  push(20, { PolicyBound: { policy_id: 2, submission_id: 2, insured_id: 2, panel: [[1, 0.5], [2, 0.5]], premium: 100, sum_insured: 1000 } });
  // policy 3: insured 3 (US-Gulf, attritional only) — should NOT count toward cat_aggregate.
  push(30, { PolicyBound: { policy_id: 3, submission_id: 3, insured_id: 3, panel: [[2, 1.0]], premium: 100, sum_insured: 1000 } });
  push(359, { YearEnd: { year: 1 } });

  // Y2: policies 1 and 2 expire mid-year (before renewal). Renewals bind anew.
  push(360, { YearStart: { year: 2 } });
  push(370, { PolicyExpired: { policy_id: 1 } });
  push(380, { PolicyExpired: { policy_id: 2 } });
  push(385, { PolicyExpired: { policy_id: 3 } });
  // Renewals — keep same risks (use most recent CR per insured).
  push(385, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: ["WindstormAtlantic", "Attritional"] } } });
  push(385, { CoverageRequested: { insured_id: 2, risk: { sum_insured: 1000, territory: "US-SE", perils_covered: ["WindstormAtlantic", "Attritional"] } } });
  // policy 11: insured 1 bound to insurer 2 (line 1.0). insurer 1 cat_agg = 0 here.
  push(400, { PolicyBound: { policy_id: 11, submission_id: 11, insured_id: 1, panel: [[2, 1.0]], premium: 100, sum_insured: 1000 } });
  // policy 12: insured 2 split 1+2 each 0.5.
  push(410, { PolicyBound: { policy_id: 12, submission_id: 12, insured_id: 2, panel: [[1, 0.5], [2, 0.5]], premium: 100, sum_insured: 1000 } });
  push(719, { YearEnd: { year: 2 } });

  // Y3: nothing new; policies still in force at YE.
  push(720, { YearStart: { year: 3 } });
  push(1079, { YearEnd: { year: 3 } });

  // Y4: policy 11 expires. insurer 2 cat_agg drops accordingly.
  push(1080, { YearStart: { year: 4 } });
  push(1090, { PolicyExpired: { policy_id: 11 } });
  push(1439, { YearEnd: { year: 4 } });

  return lines.join("\n");
}

test("prepPanel5Data — emits analysis years and one util series per insurer", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db);
  assert.equal(data.warmupYears, 1);
  assert.deepEqual(data.years, [2, 3, 4]);
  const ids = data.utilSeries.map((s) => s.insurerId).sort((a, b) => a - b);
  assert.deepEqual(ids, [1, 2]);
});

test("prepPanel5Data — cat_aggregate accumulates only on cat-covered binds, scaled by line_share", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db, { includeWarmup: true });
  const byId = new Map(data.utilSeries.map((s) => [s.insurerId, s]));
  const at = (id, y) => byId.get(id).points.find((p) => p.year === y).cat_aggregate;
  // Y1 year-end:
  // insurer 1: policy 1 (1.0 × 1000) + policy 2 (0.5 × 1000) = 1500
  // insurer 2: policy 2 (0.5 × 1000) + policy 3 (attritional, ignored) = 500
  assert.equal(at(1, 1), 1500);
  assert.equal(at(2, 1), 500);
});

test("prepPanel5Data — PolicyExpired releases cat_aggregate", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db, { includeWarmup: true });
  const byId = new Map(data.utilSeries.map((s) => [s.insurerId, s]));
  const at = (id, y) => byId.get(id).points.find((p) => p.year === y).cat_aggregate;
  // Y2 year-end (after expiries + new binds):
  // insurer 1: only policy 12 0.5 × 1000 = 500
  // insurer 2: policy 11 (1.0 × 1000) + policy 12 (0.5 × 1000) = 1500
  assert.equal(at(1, 2), 500);
  assert.equal(at(2, 2), 1500);
  // Y4: policy 11 expires; insurer 2 = just policy 12 (0.5 × 1000) = 500
  assert.equal(at(2, 4), 500);
});

test("prepPanel5Data — utilisation = cat_aggregate / (limitFactor × capital)", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db, { includeWarmup: true, limitFactor: 2.0 });
  const byId = new Map(data.utilSeries.map((s) => [s.insurerId, s]));
  // capital for insurer 1 at Y1 end = 1000 (initial); max = 2.0 × 1000 = 2000
  // cat_aggregate = 1500 → utilisation = 0.75
  const p = byId.get(1).points.find((p) => p.year === 1);
  assert.equal(p.capital, 1000);
  assert.equal(p.max_cat_aggregate, 2000);
  assert.ok(Math.abs(p.utilisation - 0.75) < 1e-9, `got ${p.utilisation}`);
});

test("prepPanel5Data — territoryByYear groups bind exposure (line_share × SI) per insurer per territory", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db, { includeWarmup: true });
  // Y1: insurer 1 → US-NE: 1000 (1.0×1000), US-SE: 500 (0.5×1000)
  //     insurer 2 → US-SE: 500, US-Gulf: 1000 (attritional, but territory still tracked)
  const y1 = data.territoryByYear.get(1);
  assert.equal(y1.get(1).get("US-NE"), 1000);
  assert.equal(y1.get(1).get("US-SE"), 500);
  assert.equal(y1.get(2).get("US-SE"), 500);
  assert.equal(y1.get(2).get("US-Gulf"), 1000);
});

test("prepPanel5Data — territories list contains all observed territories", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db);
  assert.deepEqual([...data.territories].sort(), ["US-Gulf", "US-NE", "US-SE"]);
});

test("prepPanel5Data — selectedYear defaults to last analysis year", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db);
  assert.equal(data.selectedYear, 4);
});

test("prepPanel5Data — selectedYear can be overridden", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel5Data(db, { selectedYear: 3 });
  assert.equal(data.selectedYear, 3);
});

test("renderPanel5 returns SVG with utilisation lines, breach reference, and territory bars", () => {
  const data = {
    warmupYears: 1,
    years: [2, 3, 4],
    utilSeries: [
      { insurerId: 1, entryYear: 1, isEntrant: false, points: [
        { year: 2, cat_aggregate: 500, capital: 1000, max_cat_aggregate: 2000, utilisation: 0.25 },
        { year: 3, cat_aggregate: 500, capital: 1000, max_cat_aggregate: 2000, utilisation: 0.25 },
        { year: 4, cat_aggregate: 500, capital: 1000, max_cat_aggregate: 2000, utilisation: 0.25 },
      ] },
      { insurerId: 2, entryYear: 1, isEntrant: false, points: [
        { year: 2, cat_aggregate: 1500, capital: 1000, max_cat_aggregate: 2000, utilisation: 0.75 },
        { year: 3, cat_aggregate: 1500, capital: 1000, max_cat_aggregate: 2000, utilisation: 0.75 },
        { year: 4, cat_aggregate: 500, capital: 1000, max_cat_aggregate: 2000, utilisation: 0.25 },
      ] },
    ],
    territoryByYear: new Map([
      [4, new Map([
        [1, new Map([["US-NE", 0], ["US-SE", 500], ["US-Gulf", 0]])],
        [2, new Map([["US-NE", 0], ["US-SE", 500], ["US-Gulf", 0]])],
      ])],
    ]),
    territories: ["US-NE", "US-SE", "US-Gulf"],
    selectedYear: 4,
  };
  const svg = renderPanel5(data, { asString: true });
  assert.equal(typeof svg, "string");
  assert.match(svg, /<svg/);
  assert.match(svg, /class="util-line"/);
  assert.match(svg, /class="breach-ref"/);
  assert.match(svg, /class="terr-bar"/);
});

test("renderPanel5 with empty data returns placeholder SVG", () => {
  const empty = {
    warmupYears: 0,
    years: [],
    utilSeries: [],
    territoryByYear: new Map(),
    territories: [],
    selectedYear: null,
  };
  const svg = renderPanel5(empty, { asString: true });
  assert.match(svg, /<svg/);
  assert.match(svg, /no data/i);
});
