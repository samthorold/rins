// Panel 2: Risk Pooling.
//
// Two side-by-side sub-panels showing per-insured ground-up loss spread
// vs. market mean. Sub-panel A is attritional losses (independent →
// pooling works, individual_cv ≫ aggregate_cv ≈ √N). Sub-panel B is
// catastrophe losses in cat-active years (correlated → pooling fails,
// CV ratio is much smaller).
//
// Two pure entry points:
//   - prepPanel2Data(db, opts) → { attr: {...}, cat: {...}, warmupYears, nInsureds }
//   - renderPanel2(data, opts) → SVG element (or string with `asString: true`)

export function percentile(sorted, p) {
  if (!sorted || sorted.length === 0) return null;
  if (sorted.length === 1) return sorted[0];
  const idx = p * (sorted.length - 1);
  const lo = Math.floor(idx);
  const hi = Math.ceil(idx);
  if (lo === hi) return sorted[lo];
  const frac = idx - lo;
  return sorted[lo] + (sorted[hi] - sorted[lo]) * frac;
}

export function summarise(values) {
  const n = values.length;
  if (n === 0) {
    return { n: 0, mean: 0, std: 0, cv: 0, p10: null, p25: null, p50: null, p75: null, p90: null, min: null, max: null };
  }
  const sorted = [...values].sort((a, b) => a - b);
  let sum = 0;
  for (const v of sorted) sum += v;
  const mean = sum / n;
  let sq = 0;
  for (const v of sorted) sq += (v - mean) * (v - mean);
  const std = Math.sqrt(sq / n);
  const cv = mean > 0 ? std / mean : 0;
  return {
    n,
    mean,
    std,
    cv,
    p10: percentile(sorted, 0.10),
    p25: percentile(sorted, 0.25),
    p50: percentile(sorted, 0.50),
    p75: percentile(sorted, 0.75),
    p90: percentile(sorted, 0.90),
    min: sorted[0],
    max: sorted[sorted.length - 1],
  };
}

function aggregateStats(rows, pooledValues) {
  // aggregate_cv = std(yearly means) / mean(yearly means).
  const means = rows.map((r) => r.mean).filter((m) => Number.isFinite(m));
  if (means.length === 0) return { individualCV: null, aggregateCV: null, cvRatio: null };
  const m = means.reduce((a, b) => a + b, 0) / means.length;
  let v = 0;
  for (const x of means) v += (x - m) * (x - m);
  const aggStd = Math.sqrt(v / means.length);
  const aggregateCV = m > 0 ? aggStd / m : 0;
  // individual_cv = pooled CV across all (insured, year) GUL values.
  let individualCV = null;
  if (pooledValues && pooledValues.length > 0) {
    const pm = pooledValues.reduce((a, b) => a + b, 0) / pooledValues.length;
    let pv = 0;
    for (const x of pooledValues) pv += (x - pm) * (x - pm);
    const pStd = Math.sqrt(pv / pooledValues.length);
    individualCV = pm > 0 ? pStd / pm : 0;
  }
  const cvRatio = individualCV !== null && aggregateCV > 0 ? individualCV / aggregateCV : null;
  return { individualCV, aggregateCV, cvRatio };
}

export function prepPanel2Data(db, opts = {}) {
  const includeWarmup = opts.includeWarmup ?? false;
  const warmupYears = db.getWarmupYears();

  // Discover the universe of insureds from PolicyBound and AssetDamage events.
  const insuredIds = new Set();
  for (const e of db.getEventsByType("PolicyBound")) {
    if (typeof e.data.insured_id === "number") insuredIds.add(e.data.insured_id);
  }
  for (const e of db.getEventsByType("AssetDamage")) {
    if (typeof e.data.insured_id === "number") insuredIds.add(e.data.insured_id);
  }
  for (const e of db.getEventsByType("CoverageRequested")) {
    if (typeof e.data.insured_id === "number") insuredIds.add(e.data.insured_id);
  }
  const nInsureds = insuredIds.size;

  // Discover all years and which years had cat events.
  const years = new Set();
  for (const e of db.events) {
    if (typeof e.year === "number") years.add(e.year);
  }
  const sortedYears = [...years].sort((a, b) => a - b);
  const filteredYears = includeWarmup ? sortedYears : sortedYears.filter((y) => y > warmupYears);

  const catActiveYears = new Set();
  for (const e of db.getEventsByType("LossEvent")) {
    if (e.data.peril === "WindstormAtlantic") catActiveYears.add(e.year);
  }

  // Per-year, per-insured GUL accumulators.
  const attrByYear = new Map();
  const catByYear = new Map();
  for (const y of filteredYears) {
    const a = new Map();
    const c = new Map();
    for (const id of insuredIds) {
      a.set(id, 0);
      c.set(id, 0);
    }
    attrByYear.set(y, a);
    catByYear.set(y, c);
  }

  for (const e of db.getEventsByType("AssetDamage")) {
    if (!filteredYears.includes(e.year)) continue;
    const id = e.data.insured_id;
    const gul = e.data.ground_up_loss ?? 0;
    if (e.data.peril === "WindstormAtlantic") {
      const c = catByYear.get(e.year);
      if (c) c.set(id, (c.get(id) ?? 0) + gul);
    } else {
      const a = attrByYear.get(e.year);
      if (a) a.set(id, (a.get(id) ?? 0) + gul);
    }
  }

  const attrPool = [];
  const attrRows = filteredYears.map((y) => {
    const values = [...attrByYear.get(y).values()];
    for (const v of values) attrPool.push(v);
    return { year: y, ...summarise(values) };
  });
  const catPool = [];
  const catRows = filteredYears
    .filter((y) => catActiveYears.has(y))
    .map((y) => {
      const values = [...catByYear.get(y).values()];
      for (const v of values) catPool.push(v);
      return { year: y, ...summarise(values) };
    });

  const sqrtN = nInsureds > 0 ? Math.sqrt(nInsureds) : 0;

  return {
    warmupYears,
    nInsureds,
    attr: { rows: attrRows, ...aggregateStats(attrRows, attrPool), sqrtN },
    cat: { rows: catRows, ...aggregateStats(catRows, catPool), sqrtN },
  };
}

// ---------- Rendering ----------

const W = 820;
const H = 320;
const SUB_GAP = 12;
const SUB_W = (W - SUB_GAP) / 2;
const M = { top: 28, right: 18, bottom: 36, left: 50 };

export function renderPanel2(data, opts = {}) {
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
  const attrRows = data.attr?.rows ?? [];
  const catRows = data.cat?.rows ?? [];
  if (attrRows.length === 0 && catRows.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel2">`
      + `<text x="${W / 2}" y="${H / 2}" text-anchor="middle" fill="#8b94a7" font-size="13">no data</text>`
      + `</svg>`;
  }

  const parts = [];
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel2" preserveAspectRatio="xMidYMid meet">`);
  parts.push(`<style>
    .panel2 { font: 11px -apple-system, system-ui, sans-serif; }
    .axis-line { stroke: #4a4f5a; stroke-width: 1; }
    .axis-text { fill: #8b94a7; }
    .gridline { stroke: #232936; stroke-width: 1; }
    .band-outer { fill: rgba(142,192,124,.10); stroke: none; }
    .band-iqr { fill: rgba(142,192,124,.22); stroke: none; }
    .band-outer-cat { fill: rgba(251,189,47,.10); stroke: none; }
    .band-iqr-cat { fill: rgba(251,189,47,.24); stroke: none; }
    .median-line { fill: none; stroke: rgba(216,222,233,.55); stroke-width: 1; stroke-dasharray: 3 3; }
    .mean-line { fill: none; stroke: #8ec07c; stroke-width: 2; }
    .mean-line-cat { fill: none; stroke: #fabd2f; stroke-width: 2; }
    .insured-dot { fill: rgba(216,222,233,.35); }
    .subtitle { fill: #d8dee9; font-size: 12px; font-weight: 600; }
    .annotation { fill: #d8dee9; font-size: 10.5px; }
    .annotation-dim { fill: #8b94a7; font-size: 10px; }
  </style>`);

  // Sub-panel A: attritional (left).
  parts.push(buildSubPanel({
    rows: attrRows,
    stats: data.attr,
    x0: 0,
    title: "A · Attritional GUL per insured",
    bandOuterClass: "band-outer",
    bandIQRClass: "band-iqr",
    meanClass: "mean-line",
    groupClass: "subpanel-attr",
  }));

  // Sub-panel B: catastrophe (right).
  parts.push(buildSubPanel({
    rows: catRows,
    stats: data.cat,
    x0: SUB_W + SUB_GAP,
    title: "B · Cat GUL per insured (cat-active years)",
    bandOuterClass: "band-outer-cat",
    bandIQRClass: "band-iqr-cat",
    meanClass: "mean-line-cat",
    groupClass: "subpanel-cat",
  }));

  parts.push(`</svg>`);
  return parts.join("");
}

function buildSubPanel({ rows, stats, x0, title, bandOuterClass, bandIQRClass, meanClass, groupClass }) {
  const parts = [];
  parts.push(`<g class="${groupClass}" transform="translate(${x0.toFixed(2)},0)">`);
  parts.push(`<text class="subtitle" x="${M.left}" y="14">${title}</text>`);

  const innerW = SUB_W - M.left - M.right;
  const innerH = H - M.top - M.bottom;
  const left = M.left;
  const top = M.top;
  const right = left + innerW;
  const bottom = top + innerH;

  if (rows.length === 0) {
    parts.push(`<text class="annotation-dim" x="${(M.left + innerW / 2).toFixed(2)}" y="${(top + innerH / 2).toFixed(2)}" text-anchor="middle">no cat-active years</text>`);
    parts.push(`</g>`);
    return parts.join("");
  }

  const years = rows.map((r) => r.year);
  const yMin = years[0];
  const yMax = years[years.length - 1];
  const xSpan = Math.max(1, yMax - yMin);
  const xOf = rows.length === 1
    ? () => left + innerW / 2
    : (y) => left + ((y - yMin) / xSpan) * innerW;

  let vMax = 0;
  for (const r of rows) {
    if (Number.isFinite(r.max) && r.max > vMax) vMax = r.max;
  }
  if (vMax <= 0) vMax = 1;
  vMax *= 1.1;
  const yOf = (v) => top + innerH * (1 - v / vMax);

  // Bands (p10–p90 outer, p25–p75 IQR).
  if (rows.length > 1) {
    parts.push(buildBand(rows, "p10", "p90", xOf, yOf, bandOuterClass));
    parts.push(buildBand(rows, "p25", "p75", xOf, yOf, bandIQRClass));
    parts.push(buildLine(rows, "p50", xOf, yOf, "median-line"));
    parts.push(buildLine(rows, "mean", xOf, yOf, meanClass));
  } else {
    // Single point: draw whisker + dots.
    const r = rows[0];
    const x = xOf(r.year);
    if (Number.isFinite(r.p10) && Number.isFinite(r.p90)) {
      parts.push(`<line class="${bandOuterClass}" x1="${x.toFixed(2)}" y1="${yOf(r.p10).toFixed(2)}" x2="${x.toFixed(2)}" y2="${yOf(r.p90).toFixed(2)}" stroke="rgba(142,192,124,.4)" stroke-width="6" />`);
    }
    parts.push(`<circle class="insured-dot" cx="${x.toFixed(2)}" cy="${yOf(r.mean).toFixed(2)}" r="4" />`);
  }

  // Frame + axes.
  parts.push(`<line class="axis-line" x1="${left}" y1="${bottom}" x2="${right}" y2="${bottom}" />`);
  parts.push(`<line class="axis-line" x1="${left}" y1="${top}" x2="${left}" y2="${bottom}" />`);
  // Y-axis ticks.
  for (let i = 0; i <= 4; i++) {
    const v = (i / 4) * vMax;
    const y = yOf(v);
    parts.push(`<line class="gridline" x1="${left}" y1="${y.toFixed(2)}" x2="${right}" y2="${y.toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${(left - 6).toFixed(2)}" y="${(y + 3).toFixed(2)}" text-anchor="end">${formatMoney(v)}</text>`);
  }
  // X-axis ticks.
  const span = yMax - yMin;
  const stride = span <= 10 ? 1 : span <= 30 ? 5 : span <= 100 ? 10 : 20;
  for (let y = yMin; y <= yMax; y++) {
    if ((y - yMin) % stride !== 0 && y !== yMax) continue;
    const x = xOf(y);
    parts.push(`<line class="axis-line" x1="${x.toFixed(2)}" y1="${bottom}" x2="${x.toFixed(2)}" y2="${(bottom + 4).toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${x.toFixed(2)}" y="${(bottom + 16).toFixed(2)}" text-anchor="middle">${y}</text>`);
  }

  // CV-ratio annotation.
  const ind = stats?.individualCV;
  const agg = stats?.aggregateCV;
  const ratio = stats?.cvRatio;
  const sqrtN = stats?.sqrtN ?? 0;
  const annoY = top + 14;
  const ratioStr = ratio === null || ratio === undefined ? "n/a" : ratio.toFixed(2);
  const indStr = ind === null || ind === undefined ? "—" : ind.toFixed(3);
  const aggStr = agg === null || agg === undefined ? "—" : agg.toFixed(3);
  const sqrtStr = sqrtN > 0 ? sqrtN.toFixed(2) : "—";
  parts.push(`<text class="annotation" x="${right.toFixed(2)}" y="${annoY.toFixed(2)}" text-anchor="end">CV ratio = ${ratioStr} (target √N ≈ ${sqrtStr})</text>`);
  parts.push(`<text class="annotation-dim" x="${right.toFixed(2)}" y="${(annoY + 12).toFixed(2)}" text-anchor="end">individual ${indStr} / aggregate ${aggStr}</text>`);

  parts.push(`</g>`);
  return parts.join("");
}

function buildLine(rows, key, xOf, yOf, cls) {
  let d = "";
  let pen = false;
  for (const r of rows) {
    const v = r[key];
    if (!Number.isFinite(v)) {
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

function buildBand(rows, lowKey, highKey, xOf, yOf, cls) {
  const useable = rows.filter((r) => Number.isFinite(r[lowKey]) && Number.isFinite(r[highKey]));
  if (useable.length === 0) return "";
  let top = "";
  let bot = "";
  for (let i = 0; i < useable.length; i++) {
    const r = useable[i];
    const x = xOf(r.year).toFixed(2);
    const yHi = yOf(r[highKey]).toFixed(2);
    top += i === 0 ? `M${x},${yHi}` : `L${x},${yHi}`;
  }
  for (let i = useable.length - 1; i >= 0; i--) {
    const r = useable[i];
    const x = xOf(r.year).toFixed(2);
    const yLo = yOf(r[lowKey]).toFixed(2);
    bot += `L${x},${yLo}`;
  }
  return `<path class="${cls}" d="${top}${bot}Z" />`;
}

function formatMoney(v) {
  if (v >= 1e9) return `${(v / 1e9).toFixed(1)}B`;
  if (v >= 1e6) return `${(v / 1e6).toFixed(1)}M`;
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}K`;
  return `${v.toFixed(0)}`;
}
