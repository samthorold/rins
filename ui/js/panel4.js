// Panel 4: Placement Stickiness & Market Concentration.
//
// Two sub-panels in one SVG:
//   A. Stacked policy-count share per insurer per year, with Gini overlay on
//      a right-side axis.
//   B. Relationship-score heatmap (year × insurer), score derived from
//      PolicyBound history with `score += 1.0` per panel mention and a
//      `× 0.80` decay applied each year-end.
//
//   prepPanel4Data(db, opts) → { warmupYears, years, shareSeries, giniByYear, scoresByYear }
//   renderPanel4(data, opts) → SVG element (or string when `asString: true`)

import { giniCoefficient } from "./data.js";

const DECAY = 0.80;

export function prepPanel4Data(db, opts = {}) {
  const includeWarmup = opts.includeWarmup ?? false;
  const warmupYears = db.getWarmupYears();

  const yearSet = new Set();
  for (const e of db.events) {
    if (typeof e.year === "number") yearSet.add(e.year);
  }
  const allYears = [...yearSet].sort((a, b) => a - b);
  const years = includeWarmup ? allYears : allYears.filter((y) => y > warmupYears);

  // Insurers and their entry year.
  const insurerEntry = new Map();
  for (const e of db.getEventsByType("InsurerEntered")) {
    if (!insurerEntry.has(e.data.insurer_id)) {
      insurerEntry.set(e.data.insurer_id, e.year);
    }
  }
  const insurerIds = [...insurerEntry.keys()].sort((a, b) => a - b);

  // Per-year, per-insurer panel counts.
  const countsByYear = new Map();
  for (const y of allYears) {
    const m = new Map();
    for (const id of insurerIds) m.set(id, 0);
    countsByYear.set(y, m);
  }
  for (const e of db.getEventsByType("PolicyBound")) {
    const m = countsByYear.get(e.year);
    if (!m) continue;
    const panel = e.data.panel ?? [];
    for (const [id] of panel) {
      if (!m.has(id)) m.set(id, 0);
      m.set(id, m.get(id) + 1);
    }
  }

  // Share series — one per insurer, restricted to selected years.
  const shareSeries = insurerIds.map((id) => {
    const entryYear = insurerEntry.get(id);
    const isEntrant = entryYear > warmupYears;
    const points = years.map((y) => ({ year: y, count: countsByYear.get(y).get(id) ?? 0 }));
    return { insurerId: id, entryYear, isEntrant, points };
  });

  // Gini per analysis year over all *ever-active* insurers (so non-binders
  // contribute zeros and concentration shows up correctly).
  const giniByYear = years.map((y) => {
    const m = countsByYear.get(y);
    const values = [];
    for (const id of insurerIds) {
      // Only include insurers that had entered by year y.
      if (insurerEntry.get(id) <= y) values.push(m.get(id) ?? 0);
    }
    return { year: y, gini: giniCoefficient(values) };
  });

  // Relationship scores — replay all years (incl. warmup) so analysis-year
  // scores reflect prior history, then restrict the report to `years`.
  const liveScores = new Map();
  for (const id of insurerIds) liveScores.set(id, 0);
  const scoresByYearAll = new Map();
  for (const y of allYears) {
    // +1 per bind that names the insurer in its panel.
    const m = countsByYear.get(y);
    for (const id of insurerIds) {
      const c = m.get(id) ?? 0;
      if (c > 0) liveScores.set(id, liveScores.get(id) + c);
    }
    // Snapshot at year-end (post-bind, pre-decay so the score reflects the
    // year's accumulated relationship strength).
    const snap = {};
    for (const id of insurerIds) snap[id] = liveScores.get(id);
    scoresByYearAll.set(y, snap);
    // Decay for next year.
    for (const id of insurerIds) liveScores.set(id, liveScores.get(id) * DECAY);
  }
  const scoresByYear = years.map((y) => ({ year: y, scores: scoresByYearAll.get(y) }));

  return { warmupYears, years, shareSeries, giniByYear, scoresByYear };
}

// ---------- Rendering ----------

const W = 820;
const H = 480;
const M = { top: 28, right: 56, bottom: 32, left: 56 };
const SPLIT = 0.62; // top sub-panel uses 62% of inner height.

const PALETTE = [
  "#8ec07c", "#83a598", "#fabd2f", "#d3869b",
  "#fe8019", "#b8bb26", "#458588", "#689d6a",
  "#cc241d", "#d65d0e", "#928374", "#076678",
];
const ENTRANT_PALETTE = [
  "#fb4934", "#d3869b", "#b16286", "#cc241d",
];

export function renderPanel4(data, opts = {}) {
  const asString = opts.asString ?? false;
  const svg = buildSvg(data);
  if (asString) return svg;
  if (typeof DOMParser !== "undefined") {
    const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
    return doc.documentElement;
  }
  return svg;
}

function buildSvg(data) {
  const years = data.years ?? [];
  const series = data.shareSeries ?? [];
  if (years.length === 0 || series.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel4">`
      + `<text x="${W / 2}" y="${H / 2}" text-anchor="middle" fill="#8b94a7" font-size="13">no data</text>`
      + `</svg>`;
  }

  const innerW = W - M.left - M.right;
  const innerH = H - M.top - M.bottom;
  const left = M.left;
  const top = M.top;
  const right = left + innerW;
  const aBottom = top + innerH * SPLIT - 12; // gap between sub-panels
  const bTop = top + innerH * SPLIT + 18;
  const bBottom = top + innerH;

  const yMin = years[0];
  const yMax = years[years.length - 1];
  const xSpan = Math.max(1, yMax - yMin);
  const xOf = years.length === 1
    ? () => left + innerW / 2
    : (y) => left + ((y - yMin) / xSpan) * innerW;

  const parts = [];
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel4" preserveAspectRatio="xMidYMid meet">`);
  parts.push(`<style>
    .panel4 { font: 11px -apple-system, system-ui, sans-serif; }
    .axis-line { stroke: #4a4f5a; stroke-width: 1; }
    .axis-text { fill: #8b94a7; }
    .axis-text-r { fill: #fabd2f; }
    .gridline { stroke: #232936; stroke-width: 1; }
    .share-area { stroke: rgba(15,17,21,.55); stroke-width: 0.5; }
    .gini-line { stroke: #fabd2f; stroke-width: 1.6; fill: none; }
    .gini-dot { fill: #fabd2f; }
    .rel-cell { stroke: #0f1115; stroke-width: 0.5; }
    .title { fill: #d8dee9; font-size: 12px; font-weight: 600; }
    .sub-title { fill: #d8dee9; font-size: 11px; font-weight: 600; }
    .legend-text { fill: #d8dee9; font-size: 10px; }
  </style>`);

  // ----- Sub-panel A: stacked counts -----
  parts.push(`<text class="sub-title" x="${left}" y="${(top - 12).toFixed(2)}">Market share (panel placements per year) · gini overlay</text>`);

  const stackTotals = years.map((y, i) => {
    let t = 0;
    for (const s of series) t += s.points[i]?.count ?? 0;
    return t;
  });
  let vMax = Math.max(...stackTotals, 0);
  if (vMax <= 0) vMax = 1;
  vMax = Math.ceil(vMax * 1.05);
  const aHeight = aBottom - top;
  const yOfA = (v) => top + aHeight * (1 - v / vMax);

  // Stacked bottoms/tops per series.
  const cumulative = new Array(years.length).fill(0);
  const stackBottoms = new Map();
  const stackTops = new Map();
  for (const s of series) {
    const bottoms = cumulative.slice();
    const tops = years.map((y, i) => {
      const v = s.points[i]?.count ?? 0;
      cumulative[i] += v;
      return cumulative[i];
    });
    stackBottoms.set(s.insurerId, bottoms);
    stackTops.set(s.insurerId, tops);
  }

  // Areas. Entrants get the warm-hued palette, incumbents get the cool one.
  let entrantIdx = 0;
  let incumbentIdx = 0;
  const colourOf = new Map();
  for (const s of series) {
    if (s.isEntrant) {
      colourOf.set(s.insurerId, ENTRANT_PALETTE[entrantIdx % ENTRANT_PALETTE.length]);
      entrantIdx += 1;
    } else {
      colourOf.set(s.insurerId, PALETTE[incumbentIdx % PALETTE.length]);
      incumbentIdx += 1;
    }
  }

  for (const s of series) {
    const bottoms = stackBottoms.get(s.insurerId);
    const tops = stackTops.get(s.insurerId);
    let d = "";
    for (let k = 0; k < years.length; k++) {
      const x = xOf(years[k]).toFixed(2);
      const y = yOfA(tops[k]).toFixed(2);
      d += k === 0 ? `M${x},${y}` : `L${x},${y}`;
    }
    for (let k = years.length - 1; k >= 0; k--) {
      const x = xOf(years[k]).toFixed(2);
      const y = yOfA(bottoms[k]).toFixed(2);
      d += `L${x},${y}`;
    }
    d += "Z";
    parts.push(`<path class="share-area" data-insurer="${s.insurerId}" fill="${colourOf.get(s.insurerId)}" fill-opacity="${s.isEntrant ? 0.85 : 0.78}" d="${d}" />`);
  }

  // Frame + axes for sub-panel A.
  parts.push(`<line class="axis-line" x1="${left}" y1="${aBottom}" x2="${right}" y2="${aBottom}" />`);
  parts.push(`<line class="axis-line" x1="${left}" y1="${top}" x2="${left}" y2="${aBottom}" />`);
  parts.push(`<line class="axis-line" x1="${right}" y1="${top}" x2="${right}" y2="${aBottom}" />`);
  // Left ticks (counts).
  for (let i = 0; i <= 4; i++) {
    const v = (i / 4) * vMax;
    const y = yOfA(v);
    parts.push(`<line class="gridline" x1="${left}" y1="${y.toFixed(2)}" x2="${right}" y2="${y.toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${(left - 6).toFixed(2)}" y="${(y + 3).toFixed(2)}" text-anchor="end">${Math.round(v)}</text>`);
  }
  // Right ticks (gini 0..1).
  for (let i = 0; i <= 4; i++) {
    const g = i / 4;
    const y = top + aHeight * (1 - g);
    parts.push(`<text class="axis-text-r" x="${(right + 6).toFixed(2)}" y="${(y + 3).toFixed(2)}" text-anchor="start">${g.toFixed(2)}</text>`);
  }

  // X ticks (shared with sub-panel B but draw labels under sub-panel B).
  const span = yMax - yMin;
  const stride = span <= 10 ? 1 : span <= 30 ? 5 : span <= 100 ? 10 : 20;
  for (let y = yMin; y <= yMax; y++) {
    if ((y - yMin) % stride !== 0 && y !== yMax) continue;
    const x = xOf(y);
    parts.push(`<line class="axis-line" x1="${x.toFixed(2)}" y1="${aBottom}" x2="${x.toFixed(2)}" y2="${(aBottom + 3).toFixed(2)}" />`);
  }

  // Gini line (right axis).
  const giniByYear = data.giniByYear ?? [];
  const yOfGini = (g) => top + aHeight * (1 - Math.max(0, Math.min(1, g)));
  let dGini = "";
  for (let k = 0; k < years.length; k++) {
    const g = giniByYear.find((r) => r.year === years[k]);
    if (!g) continue;
    const x = xOf(years[k]).toFixed(2);
    const y = yOfGini(g.gini).toFixed(2);
    dGini += dGini.length === 0 ? `M${x},${y}` : `L${x},${y}`;
  }
  if (dGini.length > 0) {
    parts.push(`<path class="gini-line" d="${dGini}" />`);
    for (const r of giniByYear) {
      if (r.year < yMin || r.year > yMax) continue;
      parts.push(`<circle class="gini-dot" cx="${xOf(r.year).toFixed(2)}" cy="${yOfGini(r.gini).toFixed(2)}" r="2.4" />`);
    }
  }
  // Right-axis label.
  parts.push(`<text class="axis-text-r" x="${(right + 6).toFixed(2)}" y="${(top - 6).toFixed(2)}" text-anchor="start">gini</text>`);

  // ----- Sub-panel B: relationship score heatmap -----
  parts.push(`<text class="sub-title" x="${left}" y="${(bTop - 6).toFixed(2)}">Relationship scores (×0.80 decay per year-end, +1 per panel placement)</text>`);

  const sortedSeries = [...series].sort(
    (a, b) => a.entryYear - b.entryYear || a.insurerId - b.insurerId,
  );
  const cellW = innerW / sortedSeries.length;
  const cellH = (bBottom - bTop) / years.length;

  // Find max score in window for normalisation.
  let maxScore = 0;
  for (const r of (data.scoresByYear ?? [])) {
    for (const id of Object.keys(r.scores)) {
      const v = r.scores[id] ?? 0;
      if (v > maxScore) maxScore = v;
    }
  }
  if (maxScore <= 0) maxScore = 1;

  for (let yi = 0; yi < years.length; yi++) {
    const yr = years[yi];
    const row = (data.scoresByYear ?? []).find((r) => r.year === yr);
    const scores = row ? row.scores : {};
    const yPx = bTop + yi * cellH;
    for (let ci = 0; ci < sortedSeries.length; ci++) {
      const s = sortedSeries[ci];
      const xPx = left + ci * cellW;
      const v = scores[s.insurerId] ?? 0;
      // Insurers that haven't entered yet → muted gray cell.
      const notYet = s.entryYear > yr;
      const intensity = notYet ? 0 : Math.min(1, v / maxScore);
      const fill = notYet ? "#1a1d24" : heatColour(intensity);
      parts.push(`<rect class="rel-cell" data-insurer="${s.insurerId}" data-year="${yr}" x="${xPx.toFixed(2)}" y="${yPx.toFixed(2)}" width="${cellW.toFixed(2)}" height="${cellH.toFixed(2)}" fill="${fill}" />`);
    }
  }

  // Heatmap axis labels.
  // Y labels (years) — left side.
  const ystride = years.length <= 10 ? 1 : years.length <= 30 ? 5 : 10;
  for (let yi = 0; yi < years.length; yi++) {
    if (yi % ystride !== 0 && yi !== years.length - 1) continue;
    const yPx = bTop + yi * cellH + cellH / 2 + 3;
    parts.push(`<text class="axis-text" x="${(left - 6).toFixed(2)}" y="${yPx.toFixed(2)}" text-anchor="end">${years[yi]}</text>`);
  }
  // X labels (insurer ids) — under sub-panel B.
  const colStride = sortedSeries.length <= 12 ? 1 : Math.ceil(sortedSeries.length / 12);
  for (let ci = 0; ci < sortedSeries.length; ci++) {
    if (ci % colStride !== 0 && ci !== sortedSeries.length - 1) continue;
    const s = sortedSeries[ci];
    const xPx = left + ci * cellW + cellW / 2;
    const label = s.isEntrant ? `*${s.insurerId}` : `${s.insurerId}`;
    parts.push(`<text class="axis-text" x="${xPx.toFixed(2)}" y="${(bBottom + 14).toFixed(2)}" text-anchor="middle">${label}</text>`);
  }

  parts.push(`</svg>`);
  return parts.join("");
}

// Map intensity ∈ [0,1] to a dark→bright fill (matches the gruvbox accent).
function heatColour(t) {
  // Interpolate between dark panel bg and accent green/yellow.
  const a = [22, 26, 36];
  const b = [142, 192, 124];
  const r = Math.round(a[0] + (b[0] - a[0]) * t);
  const g = Math.round(a[1] + (b[1] - a[1]) * t);
  const bl = Math.round(a[2] + (b[2] - a[2]) * t);
  return `rgb(${r},${g},${bl})`;
}
