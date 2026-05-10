import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import { prepPanel3Data, renderPanel3 } from "./panel3.js";

// Fixture: 3 starting insurers, 1 warmup + 3 analysis years.
// One late entrant in year 3.
function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));
  push(0, { SimulationStart: { year_start: 1, warmup_years: 1, analysis_years: 3 } });
  for (const id of [1, 2, 3]) {
    push(0, { InsurerEntered: { insurer_id: id, initial_capital: 1000 } });
  }

  // Year 1 warmup: nothing interesting.
  push(0, { YearStart: { year: 1 } });
  push(359, { YearEnd: { year: 1 } });

  // Year 2: small claim against insurer 1 only.
  push(360, { YearStart: { year: 2 } });
  push(400, { ClaimSettled: { insurer_id: 1, policy_id: 1, claim_id: 1, amount: 200, remaining_capital: 800 } });
  push(719, { YearEnd: { year: 2 } });

  // Year 3: cat hits — insurer 1 wiped (insolvent), insurer 2 to 500, insurer 3 to 600.
  // Late entrant insurer 4 joins mid-year.
  push(720, { YearStart: { year: 3 } });
  push(800, { LossEvent: { event_id: 7, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0.4 } });
  push(801, { ClaimSettled: { insurer_id: 1, policy_id: 1, claim_id: 2, amount: 800, remaining_capital: 0 } });
  push(801, { InsurerInsolvent: { insurer_id: 1 } });
  push(801, { ClaimSettled: { insurer_id: 2, policy_id: 2, claim_id: 3, amount: 500, remaining_capital: 500 } });
  push(801, { ClaimSettled: { insurer_id: 3, policy_id: 3, claim_id: 4, amount: 400, remaining_capital: 600 } });
  push(900, { InsurerEntered: { insurer_id: 4, initial_capital: 750 } });
  push(1079, { YearEnd: { year: 3 } });

  // Year 4: no claims — capitals carry forward. Second cat hits insurer 4.
  push(1080, { YearStart: { year: 4 } });
  push(1200, { LossEvent: { event_id: 8, peril: "WindstormAtlantic", territory: "US-Gulf", damage_fraction: 0.2 } });
  push(1201, { ClaimSettled: { insurer_id: 4, policy_id: 4, claim_id: 5, amount: 250, remaining_capital: 500 } });
  push(1439, { YearEnd: { year: 4 } });

  return lines.join("\n");
}

test("prepPanel3Data — emits one series per insurer covering analysis years", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel3Data(db);
  assert.equal(data.warmupYears, 1);
  assert.deepEqual(data.years, [2, 3, 4]);
  const ids = data.series.map((s) => s.insurerId).sort((a, b) => a - b);
  assert.deepEqual(ids, [1, 2, 3, 4]);
});

test("prepPanel3Data — capital uses last ClaimSettled.remaining_capital, carries forward, zeroes after insolvency", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel3Data(db);
  const byId = new Map(data.series.map((s) => [s.insurerId, s]));

  const cap = (id, y) => {
    const pt = byId.get(id).points.find((p) => p.year === y);
    return pt ? pt.capital : null;
  };

  // Insurer 1: 800 in Y2, 0 in Y3 (insolvent), 0 in Y4.
  assert.equal(cap(1, 2), 800);
  assert.equal(cap(1, 3), 0);
  assert.equal(cap(1, 4), 0);

  // Insurer 2: no claims Y2 → carry initial 1000; 500 Y3; carry 500 Y4.
  assert.equal(cap(2, 2), 1000);
  assert.equal(cap(2, 3), 500);
  assert.equal(cap(2, 4), 500);

  // Insurer 3: carry 1000 Y2; 600 Y3; carry 600 Y4.
  assert.equal(cap(3, 2), 1000);
  assert.equal(cap(3, 3), 600);
  assert.equal(cap(3, 4), 600);

  // Insurer 4: enters year 3 → initial_capital 750 in Y3; 500 in Y4.
  assert.equal(cap(4, 3), 750);
  assert.equal(cap(4, 4), 500);
});

test("prepPanel3Data — entrant before its entry year reports null/0 (no stack contribution)", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel3Data(db);
  const s4 = data.series.find((s) => s.insurerId === 4);
  // Year 2 is before insurer 4 entered.
  const y2 = s4.points.find((p) => p.year === 2);
  assert.equal(y2.capital, 0);
  assert.equal(s4.entryYear, 3);
});

test("prepPanel3Data — insolvency markers emitted with year and insurer id", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel3Data(db);
  assert.deepEqual(data.insolvencies, [{ year: 3, insurerId: 1 }]);
});

test("prepPanel3Data — cat events captured per year (analysis only)", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel3Data(db);
  assert.equal(data.catEvents.length, 2);
  assert.equal(data.catEvents[0].year, 3);
  assert.equal(data.catEvents[0].territory, "US-NE");
  assert.equal(data.catEvents[1].year, 4);
  assert.equal(data.catEvents[1].territory, "US-Gulf");
});

test("prepPanel3Data — includeWarmup yields year 1 too", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel3Data(db, { includeWarmup: true });
  assert.deepEqual(data.years, [1, 2, 3, 4]);
});

test("renderPanel3 returns SVG with stacked areas, insolvency markers, and cat lines", () => {
  const data = {
    warmupYears: 1,
    years: [2, 3, 4],
    series: [
      { insurerId: 1, entryYear: 1, points: [{ year: 2, capital: 800 }, { year: 3, capital: 0 }, { year: 4, capital: 0 }] },
      { insurerId: 2, entryYear: 1, points: [{ year: 2, capital: 1000 }, { year: 3, capital: 500 }, { year: 4, capital: 500 }] },
      { insurerId: 3, entryYear: 1, points: [{ year: 2, capital: 1000 }, { year: 3, capital: 600 }, { year: 4, capital: 600 }] },
      { insurerId: 4, entryYear: 3, points: [{ year: 2, capital: 0 }, { year: 3, capital: 750 }, { year: 4, capital: 500 }] },
    ],
    insolvencies: [{ year: 3, insurerId: 1 }],
    catEvents: [
      { year: 3, day: 800, territory: "US-NE", damage_fraction: 0.4 },
      { year: 4, day: 1200, territory: "US-Gulf", damage_fraction: 0.2 },
    ],
  };
  const svg = renderPanel3(data, { asString: true });
  assert.equal(typeof svg, "string");
  assert.match(svg, /<svg/);
  assert.match(svg, /class="capital-area"/);
  assert.match(svg, /class="insolvency-marker"/);
  assert.match(svg, /class="cat-line"/);
  // Territory label appears.
  assert.match(svg, /US-NE/);
});

test("renderPanel3 with empty data returns placeholder SVG", () => {
  const empty = { warmupYears: 0, years: [], series: [], insolvencies: [], catEvents: [] };
  const svg = renderPanel3(empty, { asString: true });
  assert.match(svg, /<svg/);
  assert.match(svg, /no data/i);
});
