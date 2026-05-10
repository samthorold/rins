import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import { prepPanel2Data, renderPanel2, percentile, summarise } from "./panel2.js";

// ---------- Fixture: 4 insureds, 4 years (1 warmup + 3 analysis) ----------
//
// Year 1 (warmup): tiny attritional losses, no cat
// Year 2: attritional only — losses spread across insureds
// Year 3: cat year — all 4 insureds hit by WindstormAtlantic + small attritional
// Year 4: attritional only, different distribution
function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));
  push(0, { SimulationStart: { year_start: 1, warmup_years: 1, analysis_years: 3 } });
  // Need to ensure all 4 insureds are known. Use PolicyBound entries.
  for (const id of [1, 2, 3, 4]) {
    push(0, { InsurerEntered: { insurer_id: id, initial_capital: 1000 } });
  }

  const bind = (day, insured_id, premium = 10, sum_insured = 100) =>
    push(day, { PolicyBound: { policy_id: insured_id, submission_id: insured_id, insured_id, panel: [[1, 1.0]], premium, sum_insured } });
  const dmg = (day, insured_id, peril, gul) =>
    push(day, { AssetDamage: { insured_id, peril, ground_up_loss: gul } });

  // Y1 warmup
  push(0, { YearStart: { year: 1 } });
  for (const id of [1, 2, 3, 4]) bind(1, id);
  dmg(50, 1, "Attritional", 1);
  dmg(50, 2, "Attritional", 1);
  push(359, { YearEnd: { year: 1 } });

  // Y2 attritional only — values [1, 2, 3, 4] cents → mean 2.5
  push(360, { YearStart: { year: 2 } });
  for (const id of [1, 2, 3, 4]) bind(361, id);
  dmg(400, 1, "Attritional", 1);
  dmg(400, 2, "Attritional", 2);
  dmg(400, 3, "Attritional", 3);
  dmg(400, 4, "Attritional", 4);
  push(719, { YearEnd: { year: 2 } });

  // Y3 cat year — windstorm hits all 4 with values [10, 12, 8, 14] mean 11; plus tiny attritional [1,1,1,1]
  push(720, { YearStart: { year: 3 } });
  for (const id of [1, 2, 3, 4]) bind(721, id);
  push(800, { LossEvent: { event_id: 1, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0.1 } });
  dmg(800, 1, "WindstormAtlantic", 10);
  dmg(800, 2, "WindstormAtlantic", 12);
  dmg(800, 3, "WindstormAtlantic", 8);
  dmg(800, 4, "WindstormAtlantic", 14);
  for (const id of [1, 2, 3, 4]) dmg(810, id, "Attritional", 1);
  push(1079, { YearEnd: { year: 3 } });

  // Y4 attritional only — uniform losses [2,2,2,2]
  push(1080, { YearStart: { year: 4 } });
  for (const id of [1, 2, 3, 4]) bind(1081, id);
  for (const id of [1, 2, 3, 4]) dmg(1100, id, "Attritional", 2);
  push(1439, { YearEnd: { year: 4 } });

  return lines.join("\n");
}

test("percentile interpolates between adjacent values", () => {
  assert.equal(percentile([1, 2, 3, 4], 0.5), 2.5);
  assert.equal(percentile([1, 2, 3, 4], 0), 1);
  assert.equal(percentile([1, 2, 3, 4], 1), 4);
  assert.equal(percentile([10], 0.5), 10);
  assert.equal(percentile([], 0.5), null);
});

test("summarise computes mean, std, percentiles, cv", () => {
  const s = summarise([1, 2, 3, 4]);
  assert.equal(s.n, 4);
  assert.equal(s.mean, 2.5);
  // std (population) = sqrt(((1.5^2+0.5^2+0.5^2+1.5^2)/4)) = sqrt(1.25)
  assert.ok(Math.abs(s.std - Math.sqrt(1.25)) < 1e-9);
  assert.ok(Math.abs(s.cv - Math.sqrt(1.25) / 2.5) < 1e-9);
  assert.equal(s.p50, 2.5);
});

test("summarise handles all-zero population (cv = 0)", () => {
  const s = summarise([0, 0, 0, 0]);
  assert.equal(s.n, 4);
  assert.equal(s.mean, 0);
  assert.equal(s.std, 0);
  assert.equal(s.cv, 0);
});

test("prepPanel2Data — attritional rows include all insureds (zeros included), skip warmup", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel2Data(db);
  assert.equal(data.warmupYears, 1);
  assert.equal(data.nInsureds, 4);
  // Attritional rows: years 2, 3, 4
  const attrYears = data.attr.rows.map((r) => r.year);
  assert.deepEqual(attrYears, [2, 3, 4]);
  // Y2 mean = (1+2+3+4)/4 = 2.5
  const y2 = data.attr.rows[0];
  assert.equal(y2.n, 4);
  assert.equal(y2.mean, 2.5);
  // Y3 attritional mean = 1.0 (all four had 1)
  const y3 = data.attr.rows[1];
  assert.equal(y3.mean, 1);
  assert.equal(y3.cv, 0);
});

test("prepPanel2Data — cat rows only in cat-active years", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel2Data(db);
  const catYears = data.cat.rows.map((r) => r.year);
  assert.deepEqual(catYears, [3]);
  const y3 = data.cat.rows[0];
  assert.equal(y3.mean, 11); // (10+12+8+14)/4
  assert.equal(y3.n, 4);
});

test("prepPanel2Data — CV ratio: aggregate_cv = std(yearly means)/mean(yearly means); individual_cv = mean of per-year CV", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel2Data(db);
  // Yearly attritional means: [2.5, 1, 2]
  const m = (2.5 + 1 + 2) / 3;
  const v = ((2.5 - m) ** 2 + (1 - m) ** 2 + (2 - m) ** 2) / 3;
  const aggCv = Math.sqrt(v) / m;
  assert.ok(Math.abs(data.attr.aggregateCV - aggCv) < 1e-9);
  // Individual CV per year: y2 cv = sqrt(1.25)/2.5; y3 cv = 0; y4 cv = 0
  const expectedIndCV = (Math.sqrt(1.25) / 2.5 + 0 + 0) / 3;
  assert.ok(Math.abs(data.attr.individualCV - expectedIndCV) < 1e-9);
  // CV ratio
  assert.ok(Math.abs(data.attr.cvRatio - expectedIndCV / aggCv) < 1e-9);
  assert.ok(Math.abs(data.attr.sqrtN - 2) < 1e-9); // sqrt(4)
});

test("prepPanel2Data — supports includeWarmup", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel2Data(db, { includeWarmup: true });
  assert.deepEqual(data.attr.rows.map((r) => r.year), [1, 2, 3, 4]);
});

test("renderPanel2 returns SVG with both sub-panels and annotations", () => {
  const data = {
    warmupYears: 1,
    nInsureds: 4,
    attr: {
      rows: [
        { year: 2, n: 4, mean: 2.5, std: 1.118, cv: 0.447, p10: 1, p25: 1.25, p50: 2.5, p75: 3.75, p90: 4, min: 1, max: 4 },
        { year: 3, n: 4, mean: 1, std: 0, cv: 0, p10: 1, p25: 1, p50: 1, p75: 1, p90: 1, min: 1, max: 1 },
      ],
      individualCV: 0.224,
      aggregateCV: 0.354,
      cvRatio: 0.633,
      sqrtN: 2,
    },
    cat: {
      rows: [
        { year: 3, n: 4, mean: 11, std: 2.236, cv: 0.203, p10: 8.6, p25: 9.5, p50: 11, p75: 12.5, p90: 13.4, min: 8, max: 14 },
      ],
      individualCV: 0.203,
      aggregateCV: 0,
      cvRatio: null,
      sqrtN: 2,
    },
  };
  const svg = renderPanel2(data, { asString: true });
  assert.equal(typeof svg, "string");
  assert.match(svg, /<svg/);
  // Two sub-panels with distinct group classes.
  assert.match(svg, /class="subpanel-attr"/);
  assert.match(svg, /class="subpanel-cat"/);
  // Mean trace and band.
  assert.match(svg, /class="mean-line"/);
  assert.match(svg, /class="band-iqr"/);
  // CV ratio annotation.
  assert.match(svg, /CV ratio/i);
});

test("renderPanel2 with empty data returns placeholder SVG", () => {
  const empty = {
    warmupYears: 0,
    nInsureds: 0,
    attr: { rows: [], individualCV: null, aggregateCV: null, cvRatio: null, sqrtN: 0 },
    cat: { rows: [], individualCV: null, aggregateCV: null, cvRatio: null, sqrtN: 0 },
  };
  const svg = renderPanel2(empty, { asString: true });
  assert.match(svg, /<svg/);
  assert.match(svg, /no data/i);
});
