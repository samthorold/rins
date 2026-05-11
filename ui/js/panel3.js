// Panel 3: Capital Crisis Waterfall.
//
// Stacked area of per-insurer capital across analysis years, overlaid with
// insolvency markers and cat-event vertical lines. The visual story: shared
// catastrophe occurrences depleting many insurers at once.
//
//   - prepPanel3Data(db, opts) → { warmupYears, years, series, insolvencies, catEvents }
//   - renderPanel3(data, opts) → SVG element (or string with `asString: true`)

export function prepPanel3Data(db, opts = {}) {
  const includeWarmup = opts.includeWarmup ?? false;
  const warmupYears = db.getWarmupYears();

  const yearSet = new Set();
  for (const e of db.events) {
    if (typeof e.year === "number") yearSet.add(e.year);
  }
  const allYears = [...yearSet].sort((a, b) => a - b);
  const years = includeWarmup ? allYears : allYears.filter((y) => y > warmupYears);

  // Discover insurers and their entry year + initial capital.
  const insurerEntry = new Map(); // id → { entryYear, initialCapital }
  for (const e of db.getEventsByType("InsurerEntered")) {
    if (!insurerEntry.has(e.data.insurer_id)) {
      insurerEntry.set(e.data.insurer_id, {
        entryYear: e.year,
        initialCapital: e.data.initial_capital ?? 0,
      });
    }
  }

  const insolventYear = new Map(); // id → year insolvency declared
  for (const e of db.getEventsByType("InsurerInsolvent")) {
    if (!insolventYear.has(e.data.insurer_id)) {
      insolventYear.set(e.data.insurer_id, e.year);
    }
  }

  // Walk the event log once, tracking running capital per insurer.
  // Snapshot at the end of each year. Carry-forward if no claim that year.
  const insurerIds = [...insurerEntry.keys()].sort((a, b) => a - b);
  const capitalNow = new Map();
  const yearSnapshots = new Map(); // year → Map<id, capital>

  // Pre-stage: build a sorted list of all years (incl. warmup) we care about
  // for snapshot points so we can fill carry-forward across the warmup gap.
  for (const y of allYears) yearSnapshots.set(y, new Map());

  let cursorYearIdx = 0;
  const snapshotThrough = (year) => {
    while (cursorYearIdx < allYears.length && allYears[cursorYearIdx] <= year) {
      const y = allYears[cursorYearIdx];
      const snap = yearSnapshots.get(y);
      for (const id of insurerIds) {
        const entry = insurerEntry.get(id);
        if (entry.entryYear > y) {
          snap.set(id, 0);
        } else {
          snap.set(id, capitalNow.get(id) ?? entry.initialCapital);
        }
      }
      cursorYearIdx += 1;
    }
  };

  for (const e of db.events) {
    // Snapshot any years that have fully elapsed before this event's year.
    snapshotThrough(e.year - 1);

    if (e.type === "InsurerEntered") {
      const id = e.data.insurer_id;
      if (!capitalNow.has(id)) {
        capitalNow.set(id, e.data.initial_capital ?? 0);
      }
    } else if (e.type === "ClaimSettled") {
      const id = e.data.insurer_id;
      if (typeof e.data.remaining_capital === "number") {
        capitalNow.set(id, e.data.remaining_capital);
      }
    } else if (e.type === "InsurerInsolvent") {
      capitalNow.set(e.data.insurer_id, 0);
    }
  }
  // Snapshot remaining years.
  snapshotThrough(allYears[allYears.length - 1] ?? -1);

  // Build series, restricted to selected years.
  const series = insurerIds.map((id) => {
    const entry = insurerEntry.get(id);
    const points = years.map((y) => {
      const snap = yearSnapshots.get(y);
      return { year: y, capital: snap ? snap.get(id) ?? 0 : 0 };
    });
    return { insurerId: id, entryYear: entry.entryYear, points };
  });

  const insolvencies = [];
  for (const e of db.getEventsByType("InsurerInsolvent")) {
    if (!includeWarmup && e.year <= warmupYears) continue;
    insolvencies.push({ year: e.year, insurerId: e.data.insurer_id });
  }

  const catEvents = [];
  for (const e of db.getEventsByType("LossEvent")) {
    if (e.data.peril !== "WindstormAtlantic") continue;
    if (!includeWarmup && e.year <= warmupYears) continue;
    catEvents.push({
      year: e.year,
      day: e.day,
      territory: e.data.territory ?? "",
      damage_fraction: e.data.damage_fraction ?? 0,
    });
  }

  return { warmupYears, years, series, insolvencies, catEvents };
}

// ---------- Rendering ----------

const W = 820;
const H = 360;
const M = { top: 28, right: 18, bottom: 36, left: 60 };

const PALETTE = [
  "#8ec07c", "#83a598", "#fabd2f", "#d3869b", "#fb4934",
  "#fe8019", "#b8bb26", "#458588", "#cc241d", "#689d6a",
  "#d65d0e", "#928374",
];

export function renderPanel3(data, opts = {}) {
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
  const series = data.series ?? [];
  if (years.length === 0 || series.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel3">`
      + `<text x="${W / 2}" y="${H / 2}" text-anchor="middle" fill="#8b94a7" font-size="13">no data</text>`
      + `</svg>`;
  }

  const innerW = W - M.left - M.right;
  const innerH = H - M.top - M.bottom;
  const left = M.left;
  const top = M.top;
  const right = left + innerW;
  const bottom = top + innerH;

  const yMin = years[0];
  const yMax = years[years.length - 1];
  const xSpan = Math.max(1, yMax - yMin);
  const xOf = years.length === 1
    ? () => left + innerW / 2
    : (y) => left + ((y - yMin) / xSpan) * innerW;

  // Compute stacked totals to pick capital scale.
  const totalsByYear = years.map((y) => {
    let t = 0;
    for (const s of series) {
      const pt = s.points.find((p) => p.year === y);
      if (pt && Number.isFinite(pt.capital)) t += pt.capital;
    }
    return t;
  });
  let vMax = Math.max(...totalsByYear, 0);
  if (vMax <= 0) vMax = 1;
  vMax *= 1.05;
  const yOf = (v) => top + innerH * (1 - v / vMax);

  const parts = [];
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel3" preserveAspectRatio="xMidYMid meet">`);
  parts.push(`<style>
    .panel3 { font: 11px -apple-system, system-ui, sans-serif; }
    .axis-line { stroke: #4a4f5a; stroke-width: 1; }
    .axis-text { fill: #8b94a7; }
    .gridline { stroke: #232936; stroke-width: 1; }
    .capital-area { stroke: rgba(15,17,21,.55); stroke-width: 0.5; }
    .insolvency-marker { stroke: #fb4934; stroke-width: 2; fill: none; }
    .insolvency-text { fill: #fb4934; font-size: 10px; font-weight: 600; }
    .cat-line { stroke: #fb4934; stroke-width: 1; stroke-dasharray: 4 3; opacity: .8; }
    .cat-text { fill: #fb4934; font-size: 10px; }
    .legend-text { fill: #d8dee9; font-size: 10.5px; }
    .title { fill: #d8dee9; font-size: 12px; font-weight: 600; }
  </style>`);

  // Compute stacked bands per insurer: bottom and top per year.
  const stackBottoms = new Map(); // id → array of bottoms aligned with years
  const stackTops = new Map();
  const cumulative = new Array(years.length).fill(0);
  for (const s of series) {
    const bottoms = cumulative.slice();
    const tops = years.map((y, i) => {
      const pt = s.points.find((p) => p.year === y);
      const v = pt && Number.isFinite(pt.capital) ? pt.capital : 0;
      cumulative[i] += v;
      return cumulative[i];
    });
    stackBottoms.set(s.insurerId, bottoms);
    stackTops.set(s.insurerId, tops);
  }

  // Areas.
  for (let i = 0; i < series.length; i++) {
    const s = series[i];
    const color = PALETTE[i % PALETTE.length];
    const bottoms = stackBottoms.get(s.insurerId);
    const tops = stackTops.get(s.insurerId);
    let d = "";
    for (let k = 0; k < years.length; k++) {
      const x = xOf(years[k]).toFixed(2);
      const y = yOf(tops[k]).toFixed(2);
      d += k === 0 ? `M${x},${y}` : `L${x},${y}`;
    }
    for (let k = years.length - 1; k >= 0; k--) {
      const x = xOf(years[k]).toFixed(2);
      const y = yOf(bottoms[k]).toFixed(2);
      d += `L${x},${y}`;
    }
    d += "Z";
    parts.push(`<path class="capital-area" data-insurer="${s.insurerId}" fill="${color}" fill-opacity="0.78" d="${d}" />`);
  }

  // Frame + axes.
  parts.push(`<line class="axis-line" x1="${left}" y1="${bottom}" x2="${right}" y2="${bottom}" />`);
  parts.push(`<line class="axis-line" x1="${left}" y1="${top}" x2="${left}" y2="${bottom}" />`);
  // Y ticks.
  for (let i = 0; i <= 4; i++) {
    const v = (i / 4) * vMax;
    const y = yOf(v);
    parts.push(`<line class="gridline" x1="${left}" y1="${y.toFixed(2)}" x2="${right}" y2="${y.toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${(left - 6).toFixed(2)}" y="${(y + 3).toFixed(2)}" text-anchor="end">${formatMoney(v)}</text>`);
  }
  // X ticks.
  const span = yMax - yMin;
  const stride = span <= 10 ? 1 : span <= 30 ? 5 : span <= 100 ? 10 : 20;
  for (let y = yMin; y <= yMax; y++) {
    if ((y - yMin) % stride !== 0 && y !== yMax) continue;
    const x = xOf(y);
    parts.push(`<line class="axis-line" x1="${x.toFixed(2)}" y1="${bottom}" x2="${x.toFixed(2)}" y2="${(bottom + 4).toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${x.toFixed(2)}" y="${(bottom + 16).toFixed(2)}" text-anchor="middle">${y}</text>`);
  }

  // Cat-event vertical lines (drawn behind markers but in front of areas).
  // Group multiple events per year into a single line with stacked labels.
  const catByYear = new Map();
  for (const ev of (data.catEvents ?? [])) {
    if (ev.year < yMin || ev.year > yMax) continue;
    let arr = catByYear.get(ev.year);
    if (!arr) { arr = []; catByYear.set(ev.year, arr); }
    arr.push(ev);
  }
  for (const [year, evs] of catByYear) {
    const x = xOf(year).toFixed(2);
    parts.push(`<line class="cat-line" x1="${x}" y1="${top}" x2="${x}" y2="${bottom}" />`);
    let ty = top + 10;
    for (const ev of evs) {
      const pct = Math.round((ev.damage_fraction ?? 0) * 100);
      const label = `${ev.territory} ${pct}%`;
      parts.push(`<text class="cat-text" x="${(parseFloat(x) + 3).toFixed(2)}" y="${ty.toFixed(2)}">${escapeXml(label)}</text>`);
      ty += 11;
    }
  }

  // Insolvency markers — red X anchored to the y where this insurer's stacked
  // band drops to zero in the insolvency year (top == bottom of its band there).
  for (const ins of (data.insolvencies ?? [])) {
    if (ins.year < yMin || ins.year > yMax) continue;
    const yearIdx = years.indexOf(ins.year);
    if (yearIdx === -1) continue;
    const x = xOf(ins.year);
    const tops = stackTops.get(ins.insurerId);
    const bottoms = stackBottoms.get(ins.insurerId);
    let yMid;
    if (tops && bottoms) {
      yMid = yOf(bottoms[yearIdx]);
    } else {
      yMid = (top + bottom) / 2;
    }
    const r = 5;
    parts.push(`<g class="insolvency-marker" data-insurer="${ins.insurerId}">`);
    parts.push(`<line x1="${(x - r).toFixed(2)}" y1="${(yMid - r).toFixed(2)}" x2="${(x + r).toFixed(2)}" y2="${(yMid + r).toFixed(2)}" />`);
    parts.push(`<line x1="${(x - r).toFixed(2)}" y1="${(yMid + r).toFixed(2)}" x2="${(x + r).toFixed(2)}" y2="${(yMid - r).toFixed(2)}" />`);
    parts.push(`</g>`);
    parts.push(`<text class="insolvency-text" x="${(x + r + 2).toFixed(2)}" y="${(yMid + 3).toFixed(2)}">#${ins.insurerId}</text>`);
  }

  // Insurer-id legend (top-right inside chart area).
  const legendRowH = 12;
  const legendW = 46;
  const legendX = right - legendW;
  let legendY = top + 4;
  parts.push(`<g class="legend">`);
  for (let i = 0; i < series.length; i++) {
    const s = series[i];
    const color = PALETTE[i % PALETTE.length];
    parts.push(`<rect x="${legendX.toFixed(2)}" y="${legendY.toFixed(2)}" width="9" height="9" fill="${color}" fill-opacity="0.78" />`);
    parts.push(`<text class="legend-text" x="${(legendX + 13).toFixed(2)}" y="${(legendY + 8).toFixed(2)}">#${s.insurerId}</text>`);
    legendY += legendRowH;
  }
  parts.push(`</g>`);

  parts.push(`</svg>`);
  return parts.join("");
}

function formatMoney(v) {
  if (v >= 1e9) return `${(v / 1e9).toFixed(1)}B`;
  if (v >= 1e6) return `${(v / 1e6).toFixed(1)}M`;
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}K`;
  return `${v.toFixed(0)}`;
}

function escapeXml(s) {
  return String(s).replace(/[<>&"']/g, (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;", "'": "&apos;" }[c]));
}
