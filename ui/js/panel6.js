// Panel 6: Price Dispersion.
//
// One time-series chart per year:
//   • CV (population std / mean) of LeadQuoteIssued.premium across insurers
//   • Spread (max - min) / mean
//   Annotations:
//     - Post-cat band: the year *after* a LossEvent { peril: WindstormAtlantic }
//       (depleted-incumbent rate response bites the following year)
//     - Entrant marker: any year where an entrant insurer (entered in the
//       analysis period) issues a LeadQuoteIssued — entrants quote at the
//       cheap end, so the flag marks when their pricing actually shows up.
//
//   prepPanel6Data(db, opts) → { warmupYears, rows }
//   renderPanel6(data, opts) → SVG element (or string with `asString: true`)

export function dispersionStats(values) {
  const count = values.length;
  if (count === 0) {
    return { count: 0, mean: null, std: null, cv: null, min: null, max: null, spread: null };
  }
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  for (const v of values) {
    sum += v;
    if (v < min) min = v;
    if (v > max) max = v;
  }
  const mean = sum / count;
  if (count < 2) {
    return { count, mean, std: null, cv: null, min, max, spread: null };
  }
  let sq = 0;
  for (const v of values) {
    const d = v - mean;
    sq += d * d;
  }
  const std = Math.sqrt(sq / count);
  const cv = mean !== 0 ? std / mean : null;
  const spread = mean !== 0 ? (max - min) / mean : null;
  return { count, mean, std, cv, min, max, spread };
}

export function prepPanel6Data(db, opts = {}) {
  const includeWarmup = opts.includeWarmup ?? false;
  const warmupYears = db.getWarmupYears();

  const yearSet = new Set();
  for (const e of db.events) {
    if (typeof e.year === "number") yearSet.add(e.year);
  }
  const allYears = [...yearSet].sort((a, b) => a - b);
  const years = includeWarmup ? allYears : allYears.filter((y) => y > warmupYears);

  const premiumsByYear = new Map();
  for (const y of allYears) premiumsByYear.set(y, []);
  for (const e of db.getEventsByType("LeadQuoteIssued")) {
    const bucket = premiumsByYear.get(e.year);
    if (bucket && typeof e.data.premium === "number") bucket.push(e.data.premium);
  }

  // Entrant insurers = those whose InsurerEntered fired in the analysis period.
  const entrantInsurers = new Set();
  for (const e of db.getEventsByType("InsurerEntered")) {
    if (typeof e.year === "number" && e.year > warmupYears && e.data && typeof e.data.insurer_id === "number") {
      entrantInsurers.add(e.data.insurer_id);
    }
  }
  // A year is flagged when any entrant insurer issues a lead quote that year.
  const entrantYears = new Set();
  for (const e of db.getEventsByType("LeadQuoteIssued")) {
    if (e.data && entrantInsurers.has(e.data.insurer_id)) entrantYears.add(e.year);
  }

  // Post-cat years: depleted-incumbent rate response bites the year *after* the cat.
  const postCatYears = new Set();
  for (const e of db.getEventsByType("LossEvent")) {
    if (e.data && e.data.peril === "WindstormAtlantic" && typeof e.year === "number") {
      postCatYears.add(e.year + 1);
    }
  }

  const rows = years.map((y) => {
    const stats = dispersionStats(premiumsByYear.get(y) ?? []);
    return {
      year: y,
      count: stats.count,
      mean: stats.mean,
      cv: stats.cv,
      spread: stats.spread,
      min: stats.min,
      max: stats.max,
      hasEntrant: entrantYears.has(y),
      isPostCat: postCatYears.has(y),
    };
  });

  return { warmupYears, rows };
}

// ---------- Rendering ----------

const W = 820;
const H = 300;
const M = { top: 24, right: 24, bottom: 32, left: 50 };
const IW = W - M.left - M.right;
const IH = H - M.top - M.bottom;

export function renderPanel6(data, opts = {}) {
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
  const rows = data.rows ?? [];
  if (rows.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel6">`
      + `<text x="${W / 2}" y="${H / 2}" text-anchor="middle" fill="#8b94a7" font-size="13">no data</text>`
      + `</svg>`;
  }

  const years = rows.map((r) => r.year);
  const yMin = years[0];
  const yMax = years[years.length - 1];
  const xSpan = Math.max(1, yMax - yMin);
  const xOf = years.length === 1
    ? () => M.left + IW / 2
    : (y) => M.left + ((y - yMin) / xSpan) * IW;

  let vMaxRaw = 0;
  for (const r of rows) {
    if (typeof r.cv === "number" && r.cv > vMaxRaw) vMaxRaw = r.cv;
    if (typeof r.spread === "number" && r.spread > vMaxRaw) vMaxRaw = r.spread;
  }
  const vMax = Math.max(0.1, vMaxRaw * 1.15);
  const yOf = (v) => M.top + IH * (1 - v / vMax);

  const parts = [];
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel6" preserveAspectRatio="xMidYMid meet">`);
  parts.push(`<style>
    .panel6 { font: 11px -apple-system, system-ui, sans-serif; }
    .axis-line { stroke: #4a4f5a; stroke-width: 1; }
    .axis-text { fill: #8b94a7; }
    .gridline { stroke: #232936; stroke-width: 1; }
    .trace-cv { fill: none; stroke: #fabd2f; stroke-width: 2; }
    .trace-spread { fill: none; stroke: #83a598; stroke-width: 1.4; stroke-dasharray: 4 3; }
    .cv-ref { stroke: #fb4934; stroke-width: 1; stroke-dasharray: 2 3; fill: none; }
    .cv-ref-text { fill: #fb4934; font-size: 10px; }
    .post-cat-band { fill: rgba(251,73,52,.10); }
    .post-cat-band-label { fill: #fb4934; font-size: 10px; }
    .entrant-marker { fill: #8ec07c; stroke: #d8dee9; stroke-width: .8; }
    .entrant-label { fill: #8ec07c; font-size: 9px; }
    .legend-text { fill: #d8dee9; font-size: 11px; }
  </style>`);

  // Post-cat bands behind everything (the year *after* a cat).
  for (const r of rows) {
    if (r.isPostCat) {
      const x0 = xOf(r.year - 0.5);
      const x1 = xOf(r.year + 0.5);
      parts.push(`<rect class="post-cat-band" x="${x0.toFixed(2)}" y="${M.top}" width="${(x1 - x0).toFixed(2)}" height="${IH}" />`);
      parts.push(`<text class="post-cat-band-label" x="${((x0 + x1) / 2).toFixed(2)}" y="${(M.top + 10).toFixed(2)}" text-anchor="middle">post-cat</text>`);
    }
  }

  // CV=0.05 reference (the spec threshold for "active capital-state pricing").
  if (0.05 < vMax) {
    const refY = yOf(0.05).toFixed(2);
    parts.push(`<line class="cv-ref" x1="${M.left}" y1="${refY}" x2="${M.left + IW}" y2="${refY}" />`);
    parts.push(`<text class="cv-ref-text" x="${(M.left + IW - 4).toFixed(2)}" y="${(yOf(0.05) - 4).toFixed(2)}" text-anchor="end">CV = 0.05</text>`);
  }

  // Traces.
  parts.push(buildLine(rows, "cv", xOf, yOf, "trace-cv"));
  parts.push(buildLine(rows, "spread", xOf, yOf, "trace-spread"));

  // Entrant markers.
  for (const r of rows) {
    if (r.hasEntrant) {
      const cx = xOf(r.year);
      const cy = M.top + IH - 6;
      const s = 5;
      parts.push(`<polygon class="entrant-marker" points="${cx},${cy - s} ${cx + s},${cy} ${cx},${cy + s} ${cx - s},${cy}" />`);
      parts.push(`<text class="entrant-label" x="${cx + 6}" y="${cy + 3}">+</text>`);
    }
  }

  parts.push(buildAxes(yMin, yMax, vMax, xOf, yOf));
  parts.push(buildLegend());

  parts.push(`</svg>`);
  return parts.join("");
}

function buildLine(rows, key, xOf, yOf, cls) {
  let d = "";
  let pen = false;
  for (const r of rows) {
    const v = r[key];
    if (v === null || v === undefined || Number.isNaN(v)) {
      pen = false;
      continue;
    }
    const x = xOf(r.year).toFixed(2);
    const y = yOf(v).toFixed(2);
    d += pen ? `L${x},${y}` : `M${x},${y}`;
    pen = true;
  }
  if (d === "") return "";
  return `<path class="${cls}" d="${d}" />`;
}

function buildAxes(yMin, yMax, vMax, xOf, yOf) {
  const parts = [];
  parts.push(`<line class="axis-line" x1="${M.left}" y1="${M.top + IH}" x2="${M.left + IW}" y2="${M.top + IH}" />`);
  parts.push(`<line class="axis-line" x1="${M.left}" y1="${M.top}" x2="${M.left}" y2="${M.top + IH}" />`);

  for (let i = 0; i <= 4; i++) {
    const v = (i / 4) * vMax;
    const y = yOf(v);
    parts.push(`<line class="gridline" x1="${M.left}" y1="${y.toFixed(2)}" x2="${M.left + IW}" y2="${y.toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${M.left - 6}" y="${(y + 3).toFixed(2)}" text-anchor="end">${v.toFixed(2)}</text>`);
  }

  const span = yMax - yMin;
  const stride = span <= 10 ? 1 : span <= 30 ? 5 : span <= 100 ? 10 : 20;
  for (let y = yMin; y <= yMax; y++) {
    if ((y - yMin) % stride !== 0 && y !== yMax) continue;
    const x = xOf(y);
    parts.push(`<line class="axis-line" x1="${x.toFixed(2)}" y1="${M.top + IH}" x2="${x.toFixed(2)}" y2="${(M.top + IH + 4).toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${x.toFixed(2)}" y="${(M.top + IH + 16).toFixed(2)}" text-anchor="middle">${y}</text>`);
  }
  parts.push(`<text class="axis-text" x="${(M.left + IW / 2).toFixed(2)}" y="${(H - 4).toFixed(2)}" text-anchor="middle">year</text>`);
  return parts.join("");
}

function buildLegend() {
  const items = [
    ["trace-cv", "CV (std/mean)"],
    ["trace-spread", "spread (max-min)/mean"],
  ];
  let x = M.left + 6;
  const y = M.top - 12;
  const parts = [`<g class="legend">`];
  for (const [cls, label] of items) {
    parts.push(`<line class="${cls}" x1="${x}" y1="${y + 6}" x2="${x + 16}" y2="${y + 6}" />`);
    parts.push(`<text class="legend-text" x="${x + 20}" y="${y + 9}">${label}</text>`);
    x += 20 + label.length * 6.4 + 14;
  }
  parts.push(`</g>`);
  return parts.join("");
}
