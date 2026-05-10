import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import { prepPanel4Data, renderPanel4 } from "./panel4.js";

// Fixture: 1 warmup + 3 analysis years; 3 incumbents; one entrant in year 3.
// Each PolicyBound names a panel of insurers — every panel member counts as
// "bound" for share / relationship purposes.
function buildFixture() {
  const lines = [];
  const push = (day, event) => lines.push(JSON.stringify({ day, event }));
  push(0, { SimulationStart: { year_start: 1, warmup_years: 1, analysis_years: 3 } });
  for (const id of [1, 2, 3]) {
    push(0, { InsurerEntered: { insurer_id: id, initial_capital: 1000 } });
  }

  // Year 1 (warmup): 2 binds — incumbents 1+2, then 1+3.
  push(0, { YearStart: { year: 1 } });
  push(10, { PolicyBound: { policy_id: 1, submission_id: 1, insured_id: 1, panel: [[1, 0.5], [2, 0.5]], premium: 100, sum_insured: 1000 } });
  push(20, { PolicyBound: { policy_id: 2, submission_id: 2, insured_id: 2, panel: [[1, 0.5], [3, 0.5]], premium: 100, sum_insured: 1000 } });
  push(359, { YearEnd: { year: 1 } });

  // Year 2: 4 binds, all on incumbent 1; 1 bind on (2,3). Concentration high.
  push(360, { YearStart: { year: 2 } });
  for (let p = 0; p < 4; p++) {
    push(370 + p, { PolicyBound: { policy_id: 10 + p, submission_id: 10 + p, insured_id: 1, panel: [[1, 1.0]], premium: 100, sum_insured: 1000 } });
  }
  push(400, { PolicyBound: { policy_id: 20, submission_id: 20, insured_id: 3, panel: [[2, 0.5], [3, 0.5]], premium: 100, sum_insured: 1000 } });
  push(719, { YearEnd: { year: 2 } });

  // Year 3: entrant joins; binds spread across all 4. Gini drops.
  push(720, { YearStart: { year: 3 } });
  push(730, { InsurerEntered: { insurer_id: 4, initial_capital: 750 } });
  push(740, { PolicyBound: { policy_id: 30, submission_id: 30, insured_id: 1, panel: [[1, 0.5], [4, 0.5]], premium: 100, sum_insured: 1000 } });
  push(750, { PolicyBound: { policy_id: 31, submission_id: 31, insured_id: 2, panel: [[2, 0.5], [4, 0.5]], premium: 100, sum_insured: 1000 } });
  push(760, { PolicyBound: { policy_id: 32, submission_id: 32, insured_id: 3, panel: [[3, 1.0]], premium: 100, sum_insured: 1000 } });
  push(1079, { YearEnd: { year: 3 } });

  // Year 4: just one bind — incumbent 1 again.
  push(1080, { YearStart: { year: 4 } });
  push(1100, { PolicyBound: { policy_id: 40, submission_id: 40, insured_id: 1, panel: [[1, 1.0]], premium: 100, sum_insured: 1000 } });
  push(1439, { YearEnd: { year: 4 } });

  return lines.join("\n");
}

test("prepPanel4Data — emits analysis years and one share series per insurer", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel4Data(db);
  assert.equal(data.warmupYears, 1);
  assert.deepEqual(data.years, [2, 3, 4]);
  const ids = data.shareSeries.map((s) => s.insurerId).sort((a, b) => a - b);
  assert.deepEqual(ids, [1, 2, 3, 4]);
});

test("prepPanel4Data — share counts every panel member of a PolicyBound", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel4Data(db);
  const byId = new Map(data.shareSeries.map((s) => [s.insurerId, s]));
  const at = (id, y) => byId.get(id).points.find((p) => p.year === y).count;
  // Year 2: insurer 1 = 4, insurer 2 = 1, insurer 3 = 1, insurer 4 = 0.
  assert.equal(at(1, 2), 4);
  assert.equal(at(2, 2), 1);
  assert.equal(at(3, 2), 1);
  assert.equal(at(4, 2), 0);
  // Year 3: 1 = 1, 2 = 1, 3 = 1, 4 = 2.
  assert.equal(at(1, 3), 1);
  assert.equal(at(2, 3), 1);
  assert.equal(at(3, 3), 1);
  assert.equal(at(4, 3), 2);
  // Year 4: only insurer 1 binds.
  assert.equal(at(1, 4), 1);
  assert.equal(at(4, 4), 0);
});

test("prepPanel4Data — gini per analysis year reflects concentration", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel4Data(db);
  const byYear = new Map(data.giniByYear.map((g) => [g.year, g.gini]));
  // Year 2 is concentrated (insurer 1 dominant); year 3 spread; year 4 single.
  assert.ok(byYear.get(2) > 0.3, `expected y2 gini > 0.3, got ${byYear.get(2)}`);
  assert.ok(byYear.get(3) < byYear.get(2), `expected y3 gini < y2 gini`);
  // Year 4: only one insurer participates; gini approaches 1 (max concentration).
  assert.ok(byYear.get(4) > 0.7, `expected y4 gini high, got ${byYear.get(4)}`);
});

test("prepPanel4Data — entryYear set per insurer; entrants flagged distinctly", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel4Data(db);
  const byId = new Map(data.shareSeries.map((s) => [s.insurerId, s]));
  assert.equal(byId.get(1).entryYear, 1);
  assert.equal(byId.get(4).entryYear, 3);
  assert.equal(byId.get(4).isEntrant, true);
  assert.equal(byId.get(1).isEntrant, false);
});

test("prepPanel4Data — relationship scores apply +1 per bind, ×0.80 each year-end", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel4Data(db);
  // scores[year][insurerId] — year 2 entry: incumbents 1's score is the
  // sum over PolicyBound where insurer 1 in panel, weighted by 0.80^(2 - bind_year).
  // Year 1 binds for insurer 1: 2 (from policies 1 and 2). Year 2 binds: 4.
  // So at year=2: score(1) = 2 * 0.80 + 4 = 5.6
  const yMap = new Map(data.scoresByYear.map((r) => [r.year, r.scores]));
  const get = (y, id) => yMap.get(y)[id] ?? 0;
  assert.ok(Math.abs(get(2, 1) - 5.6) < 1e-9, `y2 insurer 1 score: ${get(2, 1)}`);
  // Year 2 insurer 2: y1 binds = 1 (policy 1), y2 binds = 1 → 1*0.8 + 1 = 1.8
  assert.ok(Math.abs(get(2, 2) - 1.8) < 1e-9, `y2 insurer 2 score: ${get(2, 2)}`);
  // Year 2 insurer 4: not yet entered → 0.
  assert.equal(get(2, 4), 0);
  // Year 3 insurer 4: 2 binds in y3 → 2.0
  assert.ok(Math.abs(get(3, 4) - 2.0) < 1e-9, `y3 insurer 4 score: ${get(3, 4)}`);
  // Year 3 insurer 1: (5.6) decay × 0.80 + 1 (one y3 bind) = 5.48
  assert.ok(Math.abs(get(3, 1) - 5.48) < 1e-9, `y3 insurer 1 score: ${get(3, 1)}`);
});

test("prepPanel4Data — includeWarmup option yields year 1 too", () => {
  const db = parseNDJSONText(buildFixture());
  const data = prepPanel4Data(db, { includeWarmup: true });
  assert.deepEqual(data.years, [1, 2, 3, 4]);
});

test("renderPanel4 returns SVG with stacked share areas, gini line, and heatmap cells", () => {
  const data = {
    warmupYears: 1,
    years: [2, 3, 4],
    shareSeries: [
      { insurerId: 1, entryYear: 1, isEntrant: false, points: [{ year: 2, count: 4 }, { year: 3, count: 1 }, { year: 4, count: 1 }] },
      { insurerId: 2, entryYear: 1, isEntrant: false, points: [{ year: 2, count: 1 }, { year: 3, count: 1 }, { year: 4, count: 0 }] },
      { insurerId: 3, entryYear: 1, isEntrant: false, points: [{ year: 2, count: 1 }, { year: 3, count: 1 }, { year: 4, count: 0 }] },
      { insurerId: 4, entryYear: 3, isEntrant: true,  points: [{ year: 2, count: 0 }, { year: 3, count: 2 }, { year: 4, count: 0 }] },
    ],
    giniByYear: [{ year: 2, gini: 0.45 }, { year: 3, gini: 0.20 }, { year: 4, gini: 0.75 }],
    scoresByYear: [
      { year: 2, scores: { 1: 5.6, 2: 1.8, 3: 1.8, 4: 0 } },
      { year: 3, scores: { 1: 5.48, 2: 2.44, 3: 2.44, 4: 2.0 } },
      { year: 4, scores: { 1: 5.384, 2: 1.952, 3: 1.952, 4: 1.6 } },
    ],
  };
  const svg = renderPanel4(data, { asString: true });
  assert.equal(typeof svg, "string");
  assert.match(svg, /<svg/);
  assert.match(svg, /class="share-area"/);
  assert.match(svg, /class="gini-line"/);
  assert.match(svg, /class="rel-cell"/);
});

test("renderPanel4 with empty data returns placeholder SVG", () => {
  const empty = { warmupYears: 0, years: [], shareSeries: [], giniByYear: [], scoresByYear: [] };
  const svg = renderPanel4(empty, { asString: true });
  assert.match(svg, /<svg/);
  assert.match(svg, /no data/i);
});
