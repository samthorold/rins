// Panel 1: Underwriting Cycle.
//
// Time-series view showing rate-on-line, combined ratio, CR EWMA on the
// left axis (percentages) and total industry capital on the right axis.
// Annotations highlight cat-heavy years, new entrants, and insolvencies.
//
// Two pure entry points:
//   - prepPanel1Data(db, opts)  → { rows, warmupYears, ewmaAlpha, expenseRatio }
//   - renderPanel1(data, opts)  → SVG element (or string with `asString: true`)
//
// The data layer (data.js) already aggregates premiums, claims, capital, etc.
// into per-year rows; this module reshapes them for the chart and computes
// the EWMA. Rendering is plain SVG — no chart library dependency.

const DEFAULT_EXPENSE_RATIO = 0.344;
const DEFAULT_EWMA_ALPHA = 1 / 3;

export function ewmaSeries(values, alpha) {
  const out = [];
  let prev = null;
  for (const v of values) {
    if (v === null || v === undefined || Number.isNaN(v)) {
      out.push(prev);
      continue;
    }
    if (prev === null) {
      prev = v;
    } else {
      prev = alpha * v + (1 - alpha) * prev;
    }
    out.push(prev);
  }
  return out;
}

export function prepPanel1Data(db, opts = {}) {
  const expenseRatio = opts.expenseRatio ?? DEFAULT_EXPENSE_RATIO;
  const ewmaAlpha = opts.ewmaAlpha ?? DEFAULT_EWMA_ALPHA;
  const includeWarmup = opts.includeWarmup ?? false;
  const warmupYears = db.getWarmupYears();
  const stats = db.getYearStats();

  const filtered = includeWarmup ? stats : stats.filter((s) => s.year > warmupYears);

  const crValues = filtered.map((s) => {
    if (!s.bound_premium || s.bound_premium === 0) return null;
    return s.claims / s.bound_premium + expenseRatio;
  });
  const ewma = ewmaSeries(crValues, ewmaAlpha);

  const rows = filtered.map((s, i) => {
    const rate_on_line = s.sum_insured > 0 ? s.bound_premium / s.sum_insured : null;
    return {
      year: s.year,
      rate_on_line,
      combined_ratio: crValues[i],
      cr_ewma: ewma[i],
      total_capital: s.total_capital,
      cat_event_count: s.cat_event_count,
      entrants: s.entrants,
      insolvencies: s.insolvencies,
    };
  });

  return { rows, warmupYears, ewmaAlpha, expenseRatio };
}

// ---------- Rendering ----------

const W = 820;
const H = 300;
const M = { top: 16, right: 60, bottom: 32, left: 50 };
const IW = W - M.left - M.right;
const IH = H - M.top - M.bottom;

export function renderPanel1(data, opts = {}) {
  const asString = opts.asString ?? false;
  const svg = buildSvg(data);
  if (asString) return svg;
  // Browser: parse the string into a real <svg> element so the panel
  // registry can mount it via replaceChildren(...).
  if (typeof DOMParser !== "undefined") {
    const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
    return doc.documentElement;
  }
  return svg;
}

function buildSvg(data) {
  const rows = data.rows ?? [];
  if (rows.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel1">`
      + `<text x="${W / 2}" y="${H / 2}" text-anchor="middle" fill="#8b94a7" font-size="13">no data</text>`
      + `</svg>`;
  }

  const years = rows.map((r) => r.year);
  const yMin = years[0];
  const yMax = years[years.length - 1];
  const xSpan = Math.max(1, yMax - yMin);
  const xOf = (y) => M.left + ((y - yMin) / xSpan) * IW;

  // Left axis: percentages. Use max of (rol*100, cr*100, cr_ewma*100, 100, 130).
  const pctValues = [];
  for (const r of rows) {
    if (r.rate_on_line !== null && r.rate_on_line !== undefined) pctValues.push(r.rate_on_line * 100);
    if (r.combined_ratio !== null && r.combined_ratio !== undefined) pctValues.push(r.combined_ratio * 100);
    if (r.cr_ewma !== null && r.cr_ewma !== undefined) pctValues.push(r.cr_ewma * 100);
  }
  const lMax = Math.max(130, ...pctValues) * 1.05;
  const lMin = 0;
  const lOf = (v) => M.top + IH * (1 - (v - lMin) / (lMax - lMin));

  // Right axis: capital, scaled in billions.
  const capValues = rows.map((r) => r.total_capital ?? 0);
  const rMaxRaw = Math.max(1, ...capValues);
  const rMax = rMaxRaw * 1.1;
  const rOf = (v) => M.top + IH * (1 - v / rMax);

  const parts = [];
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel1" preserveAspectRatio="xMidYMid meet">`);
  parts.push(`<style>
    .panel1 { font: 11px -apple-system, system-ui, sans-serif; }
    .axis-line { stroke: #4a4f5a; stroke-width: 1; }
    .axis-text { fill: #8b94a7; }
    .gridline { stroke: #232936; stroke-width: 1; }
    .trace-rol { fill: none; stroke: #fabd2f; stroke-width: 2; }
    .trace-cr { fill: none; stroke: #fb4934; stroke-width: 1.4; }
    .trace-cr-ewma { fill: none; stroke: #fb4934; stroke-width: 1.4; stroke-dasharray: 4 3; opacity: .85; }
    .trace-capital { fill: rgba(142,192,124,.18); stroke: rgba(142,192,124,.6); stroke-width: 1; }
    .cat-band { fill: rgba(251,73,52,.10); }
    .cat-band-label { fill: #fb4934; font-size: 10px; }
    .entrant-marker { fill: #8ec07c; stroke: #d8dee9; stroke-width: .8; }
    .entrant-label { fill: #8ec07c; font-size: 9px; }
    .insolvent-marker { stroke: #fb4934; stroke-width: 1.6; fill: none; }
    .cr-100-ref { stroke: #8b94a7; stroke-width: 1; stroke-dasharray: 2 3; fill: none; }
    .legend-text { fill: #d8dee9; font-size: 11px; }
  </style>`);

  // Cat bands (drawn first, sit underneath the lines).
  // A cat band spans [year - 0.5, year + 0.5] for any year with >=2 cat events.
  for (const r of rows) {
    if ((r.cat_event_count ?? 0) >= 2) {
      const x0 = xOf(r.year - 0.5);
      const x1 = xOf(r.year + 0.5);
      parts.push(`<rect class="cat-band" x="${x0.toFixed(2)}" y="${M.top}" width="${(x1 - x0).toFixed(2)}" height="${IH}" />`);
      parts.push(`<text class="cat-band-label" x="${((x0 + x1) / 2).toFixed(2)}" y="${(M.top + 10).toFixed(2)}" text-anchor="middle">Cat×${r.cat_event_count}</text>`);
    }
  }

  // Capital area (right axis).
  parts.push(buildAreaPath(rows, xOf, rOf, "trace-capital"));

  // CR=100% reference line.
  const ref100 = lOf(100);
  parts.push(`<line class="cr-100-ref" x1="${M.left}" y1="${ref100.toFixed(2)}" x2="${M.left + IW}" y2="${ref100.toFixed(2)}" />`);

  // Left-axis traces.
  parts.push(buildLinePath(rows, "rate_on_line", xOf, (v) => lOf(v * 100), "trace-rol"));
  parts.push(buildLinePath(rows, "combined_ratio", xOf, (v) => lOf(v * 100), "trace-cr"));
  parts.push(buildLinePath(rows, "cr_ewma", xOf, (v) => lOf(v * 100), "trace-cr-ewma"));

  // Entrant + insolvency markers (drawn last, on top).
  for (const r of rows) {
    if ((r.entrants ?? 0) > 0) {
      const cx = xOf(r.year);
      const cy = M.top + IH - 6;
      const s = 5;
      parts.push(`<polygon class="entrant-marker" points="${cx},${cy - s} ${cx + s},${cy} ${cx},${cy + s} ${cx - s},${cy}" />`);
      parts.push(`<text class="entrant-label" x="${cx + 6}" y="${cy + 3}">+${r.entrants}</text>`);
    }
    if ((r.insolvencies ?? 0) > 0) {
      const cx = xOf(r.year);
      const cy = M.top + IH - 6;
      const s = 5;
      parts.push(`<g class="insolvent-marker"><line x1="${cx - s}" y1="${cy - s}" x2="${cx + s}" y2="${cy + s}" /><line x1="${cx - s}" y1="${cy + s}" x2="${cx + s}" y2="${cy - s}" /></g>`);
    }
  }

  // Axes.
  parts.push(buildAxes(yMin, yMax, lMin, lMax, rMax, xOf, lOf));

  // Legend.
  parts.push(buildLegend());

  parts.push(`</svg>`);
  return parts.join("");
}

function buildLinePath(rows, key, xOf, yOf, cls) {
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

function buildAreaPath(rows, xOf, yOf, cls) {
  if (rows.length === 0) return "";
  const baseY = (M.top + IH).toFixed(2);
  const x0 = xOf(rows[0].year).toFixed(2);
  const xN = xOf(rows[rows.length - 1].year).toFixed(2);
  let d = `M${x0},${baseY}`;
  for (const r of rows) {
    const v = r.total_capital ?? 0;
    d += `L${xOf(r.year).toFixed(2)},${yOf(v).toFixed(2)}`;
  }
  d += `L${xN},${baseY}Z`;
  return `<path class="${cls}" d="${d}" />`;
}

function buildAxes(yMin, yMax, lMin, lMax, rMax, xOf, lOf) {
  const parts = [];
  // Frame.
  parts.push(`<line class="axis-line" x1="${M.left}" y1="${M.top + IH}" x2="${M.left + IW}" y2="${M.top + IH}" />`);
  parts.push(`<line class="axis-line" x1="${M.left}" y1="${M.top}" x2="${M.left}" y2="${M.top + IH}" />`);
  parts.push(`<line class="axis-line" x1="${M.left + IW}" y1="${M.top}" x2="${M.left + IW}" y2="${M.top + IH}" />`);

  // Left axis ticks (% — 4 ticks).
  for (let i = 0; i <= 4; i++) {
    const v = lMin + (i / 4) * (lMax - lMin);
    const y = lOf(v);
    parts.push(`<line class="gridline" x1="${M.left}" y1="${y.toFixed(2)}" x2="${M.left + IW}" y2="${y.toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${M.left - 6}" y="${(y + 3).toFixed(2)}" text-anchor="end">${v.toFixed(0)}%</text>`);
  }

  // Right axis (capital, in billions).
  for (let i = 0; i <= 4; i++) {
    const v = (i / 4) * rMax;
    const y = M.top + IH * (1 - v / rMax);
    const billions = v / 1e9;
    const label = billions >= 10 ? billions.toFixed(0) : billions.toFixed(1);
    parts.push(`<text class="axis-text" x="${M.left + IW + 6}" y="${(y + 3).toFixed(2)}">${label}B</text>`);
  }

  // X-axis ticks.
  const span = yMax - yMin;
  const stride = span <= 10 ? 1 : span <= 30 ? 5 : span <= 100 ? 10 : 20;
  for (let y = yMin; y <= yMax; y++) {
    if ((y - yMin) % stride !== 0 && y !== yMax) continue;
    const x = xOf(y);
    parts.push(`<line class="axis-line" x1="${x.toFixed(2)}" y1="${M.top + IH}" x2="${x.toFixed(2)}" y2="${M.top + IH + 4}" />`);
    parts.push(`<text class="axis-text" x="${x.toFixed(2)}" y="${(M.top + IH + 16).toFixed(2)}" text-anchor="middle">${y}</text>`);
  }
  parts.push(`<text class="axis-text" x="${(M.left + IW / 2).toFixed(2)}" y="${(H - 4).toFixed(2)}" text-anchor="middle">year</text>`);

  return parts.join("");
}

function buildLegend() {
  const items = [
    ["trace-rol", "rate on line"],
    ["trace-cr", "combined ratio"],
    ["trace-cr-ewma", "CR EWMA"],
    ["trace-capital", "capital (right, B)"],
  ];
  let x = M.left + 6;
  const y = M.top + 4;
  const parts = [`<g class="legend">`];
  for (const [cls, label] of items) {
    parts.push(`<line class="${cls}" x1="${x}" y1="${y + 6}" x2="${x + 16}" y2="${y + 6}" />`);
    parts.push(`<text class="legend-text" x="${x + 20}" y="${y + 9}">${label}</text>`);
    x += 20 + label.length * 6.4 + 14;
  }
  parts.push(`</g>`);
  return parts.join("");
}
