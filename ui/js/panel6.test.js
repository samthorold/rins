import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import { prepPanel6Data, renderPanel6, dispersionStats } from "./panel6.js";

function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));

  push(0, { SimulationStart: { year_start: 1, warmup_years: 1, analysis_years: 3 } });
  for (const id of [1, 2, 3]) {
    push(0, { InsurerEntered: { insurer_id: id, initial_capital: 1000 } });
  }

  // Y1 (warmup): three quotes with mean 100, identical → CV=0, spread=0.
  push(0, { YearStart: { year: 1 } });
  push(10, { LeadQuoteIssued: { submission_id: 1, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(20, { LeadQuoteIssued: { submission_id: 2, insured_id: 2, insurer_id: 2, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(30, { LeadQuoteIssued: { submission_id: 3, insured_id: 3, insurer_id: 3, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(359, { YearEnd: { year: 1 } });

  // Y2: three different premiums {80,100,120}, mean=100, std≈16.33, CV≈0.1633, spread=0.40
  push(360, { YearStart: { year: 2 } });
  push(370, { LeadQuoteIssued: { submission_id: 11, insured_id: 1, insurer_id: 1, atp: 80, premium: 80, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(380, { LeadQuoteIssued: { submission_id: 12, insured_id: 2, insurer_id: 2, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(390, { LeadQuoteIssued: { submission_id: 13, insured_id: 3, insurer_id: 3, atp: 120, premium: 120, cat_exposure_at_quote: 0, line_size: 0.25 } });
  // Cat in y2
  push(395, { LossEvent: { peril: "WindstormAtlantic", territory: "US-NE", severity: 0.1 } });
  push(719, { YearEnd: { year: 2 } });

  // Y3: new entrant arrives mid-year + quote. Premiums {200,200} with new entrant {100}.
  push(720, { YearStart: { year: 3 } });
  push(725, { InsurerEntered: { insurer_id: 4, initial_capital: 1000 } });
  push(730, { LeadQuoteIssued: { submission_id: 21, insured_id: 1, insurer_id: 1, atp: 200, premium: 200, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(740, { LeadQuoteIssued: { submission_id: 22, insured_id: 2, insurer_id: 4, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(1079, { YearEnd: { year: 3 } });

  // Y4: only one quote — CV undefined (n<2)
  push(1080, { YearStart: { year: 4 } });
  push(1090, { LeadQuoteIssued: { submission_id: 31, insured_id: 1, insurer_id: 1, atp: 150, premium: 150, cat_exposure_at_quote: 0, line_size: 0.25 } });
  push(1439, { YearEnd: { year: 4 } });

  return lines.join("\n");
}

test("dispersionStats — population std, cv, spread", () => {
  const s = dispersionStats([80, 100, 120]);
  assert.equal(s.count, 3);
  assert.equal(s.mean, 100);
  assert.ok(Math.abs(s.std - Math.sqrt((400 + 0 + 400) / 3)) < 1e-9);
  assert.ok(Math.abs(s.cv - s.std / 100) < 1e-9);
  assert.equal(s.min, 80);
  assert.equal(s.max, 120);
  assert.ok(Math.abs(s.spread - 0.4) < 1e-9);
});

test("dispersionStats — n<2 returns null cv/spread but keeps mean/count", () => {
  const s = dispersionStats([150]);
  assert.equal(s.count, 1);
  assert.equal(s.mean, 150);
  assert.equal(s.cv, null);
  assert.equal(s.spread, null);
});

test("dispersionStats — empty returns zero count and null stats", () => {
  const s = dispersionStats([]);
  assert.equal(s.count, 0);
  assert.equal(s.cv, null);
  assert.equal(s.spread, null);
});

test("prepPanel6Data — emits one row per analysis year", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel6Data(db);
  assert.equal(data.warmupYears, 1);
  assert.deepEqual(data.rows.map((r) => r.year), [2, 3, 4]);
});

test("prepPanel6Data — CV and spread computed from LeadQuoteIssued premiums", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel6Data(db);
  const y2 = data.rows.find((r) => r.year === 2);
  assert.equal(y2.count, 3);
  assert.equal(y2.mean, 100);
  assert.ok(Math.abs(y2.cv - Math.sqrt(800 / 3) / 100) < 1e-9);
  assert.ok(Math.abs(y2.spread - 0.4) < 1e-9);
});

test("prepPanel6Data — entrant year flagged when insurer entered in analysis period", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel6Data(db);
  const y2 = data.rows.find((r) => r.year === 2);
  const y3 = data.rows.find((r) => r.year === 3);
  assert.equal(y2.hasEntrant, false);
  assert.equal(y3.hasEntrant, true);
});

test("prepPanel6Data — cat year flagged from LossEvent WindstormAtlantic", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel6Data(db);
  const y2 = data.rows.find((r) => r.year === 2);
  const y3 = data.rows.find((r) => r.year === 3);
  assert.equal(y2.hasCat, true);
  assert.equal(y3.hasCat, false);
});

test("prepPanel6Data — n<2 year reports null cv/spread", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel6Data(db);
  const y4 = data.rows.find((r) => r.year === 4);
  assert.equal(y4.count, 1);
  assert.equal(y4.cv, null);
  assert.equal(y4.spread, null);
});

test("prepPanel6Data — includeWarmup keeps warmup years", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel6Data(db, { includeWarmup: true });
  assert.deepEqual(data.rows.map((r) => r.year), [1, 2, 3, 4]);
});

test("renderPanel6 returns SVG with CV and spread traces and entrant/cat marks", () => {
  const data = {
    warmupYears: 1,
    rows: [
      { year: 2, count: 3, mean: 100, cv: 0.16, spread: 0.4, min: 80, max: 120, hasEntrant: false, hasCat: true },
      { year: 3, count: 3, mean: 150, cv: 0.30, spread: 0.6, min: 100, max: 200, hasEntrant: true, hasCat: false },
      { year: 4, count: 1, mean: 150, cv: null, spread: null, min: 150, max: 150, hasEntrant: false, hasCat: false },
    ],
  };
  const svg = renderPanel6(data, { asString: true });
  assert.match(svg, /<svg/);
  assert.match(svg, /class="trace-cv"/);
  assert.match(svg, /class="trace-spread"/);
  assert.match(svg, /class="cat-band"/);
  assert.match(svg, /class="entrant-marker"/);
});

test("renderPanel6 with empty data returns placeholder SVG", () => {
  const svg = renderPanel6({ warmupYears: 0, rows: [] }, { asString: true });
  assert.match(svg, /<svg/);
  assert.match(svg, /no data/i);
});
