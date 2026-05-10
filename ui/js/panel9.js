// Panel 9: Distribution Fans (multi-run).
//
// Aggregates per-year metrics across an array of databases (one per seed) and
// emits percentile bands so the viewer can show whether a phenomenon is robust
// across seeds or a single-seed artifact.
//
//   prepPanel9Data(dbs, opts) → { runCount, warmupYears, expenseRatio, metrics: [...] }
//   renderPanel9(data, opts)  → SVG element (or string with `asString: true`)
//
// Each metric in `metrics` has shape:
//   { key, label, unit, scale, rows: [{year, p5, p25, p50, p75, p95, n}] }

const DEFAULT_EXPENSE_RATIO = 0.344;

export const METRICS = [
  { key: "rate_on_line",   label: "Rate on Line",   unit: "%",  scale: 100 },
  { key: "loss_ratio",     label: "Loss Ratio",     unit: "%",  scale: 100 },
  { key: "combined_ratio", label: "Combined Ratio", unit: "%",  scale: 100 },
  { key: "total_capital",  label: "Total Capital",  unit: "B",  scale: 1e-9 },
];

export function percentile(values, p) {
  const cleaned = [];
  for (const v of values) {
    if (v === null || v === undefined) continue;
    if (typeof v !== "number" || Number.isNaN(v)) continue;
    cleaned.push(v);
  }
  if (cleaned.length === 0) return null;
  if (cleaned.length === 1) return cleaned[0];
  cleaned.sort((a, b) => a - b);
  // Nearest-rank percentile: rank = ceil(p/100 * n), clamped into [1, n].
  let rank = Math.ceil((p / 100) * cleaned.length);
  if (rank < 1) rank = 1;
  if (rank > cleaned.length) rank = cleaned.length;
  return cleaned[rank - 1];
}

function metricValue(stat, key, expenseRatio) {
  switch (key) {
    case "rate_on_line":
      return stat.sum_insured > 0 ? stat.bound_premium / stat.sum_insured : null;
    case "loss_ratio":
      return stat.bound_premium > 0 ? stat.claims / stat.bound_premium : null;
    case "combined_ratio":
      return stat.bound_premium > 0 ? (stat.claims / stat.bound_premium) + expenseRatio : null;
    case "total_capital":
      return stat.total_capital ?? null;
    default:
      return null;
  }
}

export function prepPanel9Data(dbs, opts = {}) {
  const expenseRatio = opts.expenseRatio ?? DEFAULT_EXPENSE_RATIO;
  const includeWarmup = opts.includeWarmup ?? false;
  const warmupYears = dbs.length > 0 ? dbs[0].getWarmupYears() : 0;

  // Build per-db filtered stats.
  const perDbStats = dbs.map((db) => {
    const stats = db.getYearStats();
    const w = db.getWarmupYears();
    return includeWarmup ? stats : stats.filter((s) => s.year > w);
  });

  // Collect union of years.
  const yearSet = new Set();
  for (const stats of perDbStats) for (const s of stats) yearSet.add(s.year);
  const years = [...yearSet].sort((a, b) => a - b);

  const metrics = METRICS.map(({ key, label, unit, scale }) => {
    const rows = years.map((year) => {
      const values = [];
      for (const stats of perDbStats) {
        const s = stats.find((x) => x.year === year);
        if (!s) continue;
        const v = metricValue(s, key, expenseRatio);
        if (v !== null && v !== undefined && !Number.isNaN(v)) values.push(v);
      }
      return {
        year,
        n: values.length,
        p5:  percentile(values, 5),
        p25: percentile(values, 25),
        p50: percentile(values, 50),
        p75: percentile(values, 75),
        p95: percentile(values, 95),
      };
    });
    return { key, label, unit, scale, rows };
  });

  return { runCount: dbs.length, warmupYears, expenseRatio, metrics };
}

// ── Rendering ─────────────────────────────────────────────────────────────

const FW = 820;
const FH = 200;
const FM = { top: 18, right: 14, bottom: 28, left: 50 };
const IW = FW - FM.left - FM.right;
const IH = FH - FM.top - FM.bottom;

const STYLE = `
.p9-wrap { font: 11px -apple-system, system-ui, sans-serif; }
.p9-summary { color: var(--fg-dim); margin: 0 0 .35rem 0; font-size: .8em; }
.p9-grid { display: grid; grid-template-columns: 1fr; gap: .5rem; }
.p9-cell { background: rgba(255,255,255,.01); border: 1px solid var(--panel-border); border-radius: 3px; padding: .25rem .35rem; }
.p9-cell h4 { margin: 0 0 .15rem 0; font-size: .8em; font-weight: 600; color: var(--fg-dim); text-transform: uppercase; letter-spacing: .04em; }
.p9-band-outer { fill: rgba(142,192,124,.14); stroke: none; }
.p9-band-inner { fill: rgba(142,192,124,.32); stroke: none; }
.p9-median { fill: none; stroke: #8ec07c; stroke-width: 1.6; }
.p9-axis-line { stroke: #4a4f5a; stroke-width: 1; }
.p9-axis-text { fill: #8b94a7; }
.p9-gridline { stroke: #232936; stroke-width: 1; }
.p9-empty { fill: #8b94a7; }
`;

export function renderPanel9(data, opts = {}) {
  const asString = opts.asString === true;
  const html = buildHTML(data);
  if (asString) return html;
  const wrap = document.createElement("div");
  wrap.className = "p9-wrap";
  const styleEl = document.createElement("style");
  styleEl.textContent = STYLE;
  wrap.appendChild(styleEl);
  const host = document.createElement("div");
  host.innerHTML = html;
  while (host.firstChild) wrap.appendChild(host.firstChild);
  return wrap;
}

function buildHTML(data) {
  if (!data || data.runCount === 0) {
    return `<div class="p9-summary">no runs loaded</div>`
      + `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${FW} ${FH}">`
      + `<text class="p9-empty" x="${FW / 2}" y="${FH / 2}" text-anchor="middle">no data</text></svg>`;
  }
  const summary = `<div class="p9-summary">distribution across ${data.runCount} runs (n=${data.runCount}); bands p5–p95 / p25–p75; line p50</div>`;
  const cells = data.metrics.map((m) => `<div class="p9-cell"><h4>${escapeHtml(m.label)} (${escapeHtml(m.unit)})</h4>${metricSvg(m)}</div>`).join("");
  return summary + `<div class="p9-grid">${cells}</div>`;
}

function metricSvg(metric) {
  const rows = metric.rows.filter((r) => r.p50 !== null && r.p50 !== undefined);
  if (rows.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${FW} ${FH}">`
      + `<text class="p9-empty" x="${FW / 2}" y="${FH / 2}" text-anchor="middle">no data</text></svg>`;
  }
  const years = rows.map((r) => r.year);
  const yMin = years[0];
  const yMax = years[years.length - 1];
  const xSpan = Math.max(1, yMax - yMin);
  const xOf = (y) => FM.left + ((y - yMin) / xSpan) * IW;

  const scale = metric.scale;
  let vMin = Infinity, vMax = -Infinity;
  for (const r of rows) {
    for (const k of ["p5", "p25", "p50", "p75", "p95"]) {
      const v = r[k];
      if (v === null || v === undefined) continue;
      const sv = v * scale;
      if (sv < vMin) vMin = sv;
      if (sv > vMax) vMax = sv;
    }
  }
  if (!isFinite(vMin) || !isFinite(vMax)) { vMin = 0; vMax = 1; }
  if (vMin === vMax) { vMin -= 1; vMax += 1; }
  const pad = (vMax - vMin) * 0.08;
  vMin -= pad;
  vMax += pad;
  if (metric.key !== "total_capital" && vMin > 0) vMin = 0;

  const yOf = (v) => FM.top + IH * (1 - (v - vMin) / (vMax - vMin));

  const parts = [];
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${FW} ${FH}" preserveAspectRatio="xMidYMid meet">`);

  // Outer band p5–p95
  parts.push(buildBand(rows, "p5", "p95", xOf, yOf, scale, "p9-band-outer"));
  // Inner band p25–p75
  parts.push(buildBand(rows, "p25", "p75", xOf, yOf, scale, "p9-band-inner"));
  // Median line
  parts.push(buildLine(rows, "p50", xOf, yOf, scale, "p9-median"));

  // Axes
  parts.push(buildAxes(metric, yMin, yMax, vMin, vMax, xOf, yOf));

  parts.push(`</svg>`);
  return parts.join("");
}

function buildBand(rows, lowKey, highKey, xOf, yOf, scale, cls) {
  const valid = rows.filter((r) => r[lowKey] !== null && r[lowKey] !== undefined && r[highKey] !== null && r[highKey] !== undefined);
  if (valid.length === 0) return "";
  let d = "";
  for (let i = 0; i < valid.length; i++) {
    const r = valid[i];
    const x = xOf(r.year).toFixed(2);
    const y = yOf(r[highKey] * scale).toFixed(2);
    d += i === 0 ? `M${x},${y}` : `L${x},${y}`;
  }
  for (let i = valid.length - 1; i >= 0; i--) {
    const r = valid[i];
    const x = xOf(r.year).toFixed(2);
    const y = yOf(r[lowKey] * scale).toFixed(2);
    d += `L${x},${y}`;
  }
  d += "Z";
  return `<path class="${cls}" d="${d}" />`;
}

function buildLine(rows, key, xOf, yOf, scale, cls) {
  let d = "";
  let pen = false;
  for (const r of rows) {
    const v = r[key];
    if (v === null || v === undefined || Number.isNaN(v)) { pen = false; continue; }
    const x = xOf(r.year).toFixed(2);
    const y = yOf(v * scale).toFixed(2);
    d += pen ? `L${x},${y}` : `M${x},${y}`;
    pen = true;
  }
  if (!d) return "";
  return `<path class="${cls}" d="${d}" />`;
}

function buildAxes(metric, yMin, yMax, vMin, vMax, xOf, yOf) {
  const parts = [];
  parts.push(`<line class="p9-axis-line" x1="${FM.left}" y1="${FM.top + IH}" x2="${FM.left + IW}" y2="${FM.top + IH}" />`);
  parts.push(`<line class="p9-axis-line" x1="${FM.left}" y1="${FM.top}" x2="${FM.left}" y2="${FM.top + IH}" />`);
  for (let i = 0; i <= 4; i++) {
    const v = vMin + (i / 4) * (vMax - vMin);
    const y = yOf(v).toFixed(2);
    parts.push(`<line class="p9-gridline" x1="${FM.left}" y1="${y}" x2="${FM.left + IW}" y2="${y}" />`);
    const label = formatTick(v, metric);
    parts.push(`<text class="p9-axis-text" x="${FM.left - 6}" y="${(parseFloat(y) + 3).toFixed(2)}" text-anchor="end">${label}</text>`);
  }
  const span = yMax - yMin;
  const stride = span <= 10 ? 1 : span <= 30 ? 5 : span <= 100 ? 10 : 20;
  for (let y = yMin; y <= yMax; y++) {
    if ((y - yMin) % stride !== 0 && y !== yMax) continue;
    const x = xOf(y).toFixed(2);
    parts.push(`<line class="p9-axis-line" x1="${x}" y1="${FM.top + IH}" x2="${x}" y2="${FM.top + IH + 4}" />`);
    parts.push(`<text class="p9-axis-text" x="${x}" y="${(FM.top + IH + 14).toFixed(2)}" text-anchor="middle">${y}</text>`);
  }
  return parts.join("");
}

function formatTick(v, metric) {
  if (metric.unit === "%") return `${v.toFixed(0)}%`;
  if (metric.unit === "B") return v >= 10 ? `${v.toFixed(0)}B` : `${v.toFixed(1)}B`;
  return String(Math.round(v));
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]
  ));
}
