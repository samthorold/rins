// Panel 5: Accumulation Risk.
//
// Two sub-panels in one SVG:
//   A. Cat aggregate utilisation per insurer over time
//      (cat_aggregate / max_cat_aggregate, 0–100%, with breach reference at 100%).
//   B. Territory exposure split per insurer for the selected year.
//
//   prepPanel5Data(db, opts) → { warmupYears, years, utilSeries, territoryByYear, territories, selectedYear }
//   renderPanel5(data, opts) → SVG element (or string when `asString: true`)
//
// Cat aggregate is reconstructed from PolicyBound + PolicyExpired + InsurerInsolvent,
// joined to the most recent CoverageRequested per insured to recover risk
// territory and perils. Capital is derived from InsurerEntered, ClaimSettled,
// CapitalDistributed, and InsurerInsolvent. The "max" is configurable via
// `limitFactor` — defaults to the canonical config:
//   limitFactor = scf / (pml_200 × territory_factor)
//               = 0.30 / (0.495 × 1/3) ≈ 1.818

// Canonical default. Override via opts.limitFactor.
const DEFAULT_LIMIT_FACTOR = 1.818;

export function prepPanel5Data(db, opts = {}) {
  const includeWarmup = opts.includeWarmup ?? false;
  const limitFactor = opts.limitFactor ?? DEFAULT_LIMIT_FACTOR;
  const warmupYears = db.getWarmupYears();

  const yearSet = new Set();
  for (const e of db.events) {
    if (typeof e.year === "number") yearSet.add(e.year);
  }
  const allYears = [...yearSet].sort((a, b) => a - b);
  const years = includeWarmup ? allYears : allYears.filter((y) => y > warmupYears);

  const insurerEntry = new Map();
  for (const e of db.getEventsByType("InsurerEntered")) {
    if (!insurerEntry.has(e.data.insurer_id)) {
      insurerEntry.set(e.data.insurer_id, {
        entryYear: e.year,
        initialCapital: e.data.initial_capital ?? 0,
      });
    }
  }
  const insurerIds = [...insurerEntry.keys()].sort((a, b) => a - b);

  // Walk the event log forward, tracking:
  //   • catAggregateNow[insurer]      — current cat exposure (sum of line_share × SI)
  //   • capitalNow[insurer]
  //   • policyContrib[policy_id]      — Map<insurerId, exposureContribution>
  //                                      so PolicyExpired releases the right amount
  //   • policyTerritory[policy_id]    — territory at bind time
  //   • lastCRByInsured[insured_id]   — most recent CoverageRequested risk
  //   • territoriesSeen               — Set
  // Snapshots taken at year-end (or at the last event in a year if YearEnd absent).

  const catAggregateNow = new Map();
  const capitalNow = new Map();
  for (const id of insurerIds) {
    catAggregateNow.set(id, 0);
    capitalNow.set(id, 0);
  }
  const policyContrib = new Map();
  const lastCRByInsured = new Map();
  const territoriesSeen = new Set();

  // Per-year accumulators.
  const yearTerritory = new Map(); // year → Map<insurerId, Map<territory, exposure>>
  for (const y of allYears) {
    const inner = new Map();
    for (const id of insurerIds) inner.set(id, new Map());
    yearTerritory.set(y, inner);
  }
  const yearSnapshots = new Map(); // year → Map<id, {cat_aggregate, capital}>
  for (const y of allYears) yearSnapshots.set(y, new Map());

  let cursorYearIdx = 0;
  const snapshotThrough = (year) => {
    while (cursorYearIdx < allYears.length && allYears[cursorYearIdx] <= year) {
      const y = allYears[cursorYearIdx];
      const snap = yearSnapshots.get(y);
      for (const id of insurerIds) {
        const entry = insurerEntry.get(id);
        if (entry.entryYear > y) {
          snap.set(id, { cat_aggregate: 0, capital: 0 });
        } else {
          snap.set(id, {
            cat_aggregate: catAggregateNow.get(id) ?? 0,
            capital: capitalNow.get(id) ?? entry.initialCapital,
          });
        }
      }
      cursorYearIdx += 1;
    }
  };

  for (const e of db.events) {
    snapshotThrough(e.year - 1);
    switch (e.type) {
      case "InsurerEntered": {
        const id = e.data.insurer_id;
        if (!capitalNow.has(id) || (capitalNow.get(id) ?? 0) === 0) {
          capitalNow.set(id, e.data.initial_capital ?? 0);
        }
        if (!catAggregateNow.has(id)) catAggregateNow.set(id, 0);
        break;
      }
      case "CoverageRequested": {
        const r = e.data.risk;
        if (r) {
          lastCRByInsured.set(e.data.insured_id, r);
          if (typeof r.territory === "string") territoriesSeen.add(r.territory);
        }
        break;
      }
      case "PolicyBound": {
        const insuredId = e.data.insured_id;
        const risk = lastCRByInsured.get(insuredId);
        const territory = risk ? risk.territory : undefined;
        const isCat = !!risk && Array.isArray(risk.perils_covered)
          && risk.perils_covered.includes("WindstormAtlantic");
        const sumInsured = e.data.sum_insured ?? 0;
        const panel = e.data.panel ?? [];
        const contrib = new Map();
        const yt = yearTerritory.get(e.year);
        for (const [insurerId, lineShare] of panel) {
          const exposure = sumInsured * lineShare;
          if (isCat) {
            catAggregateNow.set(insurerId, (catAggregateNow.get(insurerId) ?? 0) + exposure);
            contrib.set(insurerId, exposure);
          }
          if (territory && yt) {
            const im = yt.get(insurerId) ?? new Map();
            im.set(territory, (im.get(territory) ?? 0) + exposure);
            yt.set(insurerId, im);
          }
        }
        if (typeof e.data.policy_id === "number" && contrib.size > 0) {
          policyContrib.set(e.data.policy_id, contrib);
        }
        break;
      }
      case "PolicyExpired": {
        const pid = e.data.policy_id;
        const contrib = policyContrib.get(pid);
        if (contrib) {
          for (const [insurerId, exposure] of contrib) {
            catAggregateNow.set(
              insurerId,
              Math.max(0, (catAggregateNow.get(insurerId) ?? 0) - exposure),
            );
          }
          policyContrib.delete(pid);
        }
        break;
      }
      case "ClaimSettled": {
        const id = e.data.insurer_id;
        if (typeof e.data.remaining_capital === "number") {
          capitalNow.set(id, e.data.remaining_capital);
        }
        break;
      }
      case "CapitalDistributed": {
        const id = e.data.insurer_id;
        if (typeof e.data.remaining_capital === "number") {
          capitalNow.set(id, e.data.remaining_capital);
        }
        break;
      }
      case "InsurerInsolvent": {
        const id = e.data.insurer_id;
        capitalNow.set(id, 0);
        catAggregateNow.set(id, 0);
        // Drop pending contributions tied to this insurer so subsequent
        // PolicyExpired events don't double-decrement.
        for (const [pid, contrib] of policyContrib) {
          if (contrib.has(id)) {
            contrib.delete(id);
            if (contrib.size === 0) policyContrib.delete(pid);
          }
        }
        break;
      }
    }
  }
  snapshotThrough(allYears[allYears.length - 1] ?? -1);

  const utilSeries = insurerIds.map((id) => {
    const entry = insurerEntry.get(id);
    const isEntrant = entry.entryYear > warmupYears;
    const points = years.map((y) => {
      const snap = yearSnapshots.get(y) ?? new Map();
      const s = snap.get(id) ?? { cat_aggregate: 0, capital: 0 };
      const max_cat_aggregate = limitFactor * (s.capital ?? 0);
      const utilisation = max_cat_aggregate > 0
        ? (s.cat_aggregate ?? 0) / max_cat_aggregate
        : 0;
      return {
        year: y,
        cat_aggregate: s.cat_aggregate ?? 0,
        capital: s.capital ?? 0,
        max_cat_aggregate,
        utilisation,
      };
    });
    return { insurerId: id, entryYear: entry.entryYear, isEntrant, points };
  });

  const territoryByYear = new Map();
  for (const y of (includeWarmup ? allYears : years)) {
    territoryByYear.set(y, yearTerritory.get(y));
  }

  const territories = [...territoriesSeen].sort();

  const selectedYear = opts.selectedYear !== undefined
    ? opts.selectedYear
    : (years.length > 0 ? years[years.length - 1] : null);

  return {
    warmupYears,
    years,
    utilSeries,
    territoryByYear,
    territories,
    selectedYear,
    limitFactor,
  };
}

// ---------- Rendering ----------

const W = 820;
const H = 480;
const M = { top: 28, right: 56, bottom: 32, left: 60 };
const SPLIT = 0.58; // top sub-panel uses 58% of inner height.

const PALETTE = [
  "#8ec07c", "#83a598", "#fabd2f", "#d3869b",
  "#fe8019", "#b8bb26", "#458588", "#689d6a",
  "#cc241d", "#d65d0e", "#928374", "#076678",
];
const ENTRANT_PALETTE = [
  "#fb4934", "#d3869b", "#b16286", "#cc241d",
];
const TERRITORY_PALETTE = [
  "#83a598", "#fabd2f", "#fb4934", "#8ec07c",
  "#d3869b", "#fe8019", "#b8bb26", "#458588",
];

export function renderPanel5(data, opts = {}) {
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
  const series = data.utilSeries ?? [];
  if (years.length === 0 || series.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel5">`
      + `<text x="${W / 2}" y="${H / 2}" text-anchor="middle" fill="#8b94a7" font-size="13">no data</text>`
      + `</svg>`;
  }

  const innerW = W - M.left - M.right;
  const innerH = H - M.top - M.bottom;
  const left = M.left;
  const top = M.top;
  const right = left + innerW;
  const aBottom = top + innerH * SPLIT - 12;
  const bTop = top + innerH * SPLIT + 18;
  const bBottom = top + innerH;

  const yMin = years[0];
  const yMax = years[years.length - 1];
  const xSpan = Math.max(1, yMax - yMin);
  const xOf = years.length === 1
    ? () => left + innerW / 2
    : (y) => left + ((y - yMin) / xSpan) * innerW;

  const parts = [];
  parts.push(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" class="panel5" preserveAspectRatio="xMidYMid meet">`);
  parts.push(`<style>
    .panel5 { font: 11px -apple-system, system-ui, sans-serif; }
    .axis-line { stroke: #4a4f5a; stroke-width: 1; }
    .axis-text { fill: #8b94a7; }
    .gridline { stroke: #232936; stroke-width: 1; }
    .util-line { fill: none; stroke-width: 1.4; opacity: .9; }
    .breach-ref { stroke: #fb4934; stroke-width: 1.2; stroke-dasharray: 5 4; fill: none; }
    .breach-text { fill: #fb4934; font-size: 10px; }
    .terr-bar { stroke: #0f1115; stroke-width: 0.5; }
    .terr-label { fill: #d8dee9; font-size: 10px; }
    .sub-title { fill: #d8dee9; font-size: 11px; font-weight: 600; }
    .legend-text { fill: #d8dee9; font-size: 10px; }
  </style>`);

  // ----- Sub-panel A: utilisation lines -----
  parts.push(`<text class="sub-title" x="${left}" y="${(top - 12).toFixed(2)}">Cat aggregate utilisation (cat_aggregate / max_cat_aggregate)</text>`);

  // Cap y-axis at max(1.05, observed_max × 1.05) so a breach above 100% is still visible.
  let observedMax = 0;
  for (const s of series) {
    for (const p of s.points) {
      if (p.utilisation > observedMax) observedMax = p.utilisation;
    }
  }
  const vMax = Math.max(1.05, observedMax * 1.05);
  const aHeight = aBottom - top;
  const yOfA = (v) => top + aHeight * (1 - v / vMax);

  // Frame.
  parts.push(`<line class="axis-line" x1="${left}" y1="${aBottom}" x2="${right}" y2="${aBottom}" />`);
  parts.push(`<line class="axis-line" x1="${left}" y1="${top}" x2="${left}" y2="${aBottom}" />`);

  // Y ticks (0%, 25%, 50%, 75%, 100%).
  const tickVals = [0, 0.25, 0.5, 0.75, 1.0];
  for (const v of tickVals) {
    if (v > vMax) continue;
    const y = yOfA(v);
    parts.push(`<line class="gridline" x1="${left}" y1="${y.toFixed(2)}" x2="${right}" y2="${y.toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${(left - 6).toFixed(2)}" y="${(y + 3).toFixed(2)}" text-anchor="end">${(v * 100).toFixed(0)}%</text>`);
  }

  // Breach reference at 100%.
  const breachY = yOfA(1.0).toFixed(2);
  parts.push(`<line class="breach-ref" x1="${left}" y1="${breachY}" x2="${right}" y2="${breachY}" />`);
  parts.push(`<text class="breach-text" x="${(right - 4).toFixed(2)}" y="${(yOfA(1.0) - 4).toFixed(2)}" text-anchor="end">breach 100%</text>`);

  // Colour assignment (entrants get warm hues, incumbents cool).
  const colourOf = new Map();
  let entrantIdx = 0;
  let incumbentIdx = 0;
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
    let d = "";
    for (let k = 0; k < s.points.length; k++) {
      const p = s.points[k];
      const x = xOf(p.year).toFixed(2);
      const y = yOfA(p.utilisation).toFixed(2);
      d += d.length === 0 ? `M${x},${y}` : `L${x},${y}`;
    }
    parts.push(`<path class="util-line" data-insurer="${s.insurerId}" stroke="${colourOf.get(s.insurerId)}" d="${d}" />`);
  }

  // X ticks for sub-panel A.
  const span = yMax - yMin;
  const stride = span <= 10 ? 1 : span <= 30 ? 5 : span <= 100 ? 10 : 20;
  for (let y = yMin; y <= yMax; y++) {
    if ((y - yMin) % stride !== 0 && y !== yMax) continue;
    const x = xOf(y);
    parts.push(`<line class="axis-line" x1="${x.toFixed(2)}" y1="${aBottom}" x2="${x.toFixed(2)}" y2="${(aBottom + 3).toFixed(2)}" />`);
    parts.push(`<text class="axis-text" x="${x.toFixed(2)}" y="${(aBottom + 14).toFixed(2)}" text-anchor="middle">${y}</text>`);
  }

  // ----- Sub-panel B: territory exposure for selected year -----
  const sel = data.selectedYear;
  const territories = data.territories ?? [];
  const tMap = (data.territoryByYear instanceof Map) ? data.territoryByYear.get(sel) : null;

  parts.push(`<text class="sub-title" x="${left}" y="${(bTop - 6).toFixed(2)}">Territory exposure split — year ${sel ?? "?"}</text>`);

  if (!tMap || territories.length === 0 || series.length === 0) {
    parts.push(`<text class="axis-text" x="${(left + innerW / 2).toFixed(2)}" y="${((bTop + bBottom) / 2).toFixed(2)}" text-anchor="middle">no exposure data for selected year</text>`);
    parts.push(`</svg>`);
    return parts.join("");
  }

  // Build per-insurer territory totals; normalise to fractions for stacked bars.
  const sortedSeries = [...series].sort(
    (a, b) => a.entryYear - b.entryYear || a.insurerId - b.insurerId,
  );
  const barCount = sortedSeries.length;
  const groupGap = 8;
  const barW = (innerW - groupGap * (barCount + 1)) / Math.max(1, barCount);
  const bHeight = bBottom - bTop;

  const territoryColour = new Map();
  territories.forEach((t, i) => {
    territoryColour.set(t, TERRITORY_PALETTE[i % TERRITORY_PALETTE.length]);
  });

  for (let bi = 0; bi < sortedSeries.length; bi++) {
    const s = sortedSeries[bi];
    const xPx = left + groupGap + bi * (barW + groupGap);
    const im = tMap.get(s.insurerId) ?? new Map();
    let total = 0;
    for (const v of im.values()) total += v;
    if (total <= 0) {
      // Empty placeholder bar.
      parts.push(`<rect class="terr-bar" data-insurer="${s.insurerId}" x="${xPx.toFixed(2)}" y="${(bBottom - 1).toFixed(2)}" width="${barW.toFixed(2)}" height="1" fill="#232936" />`);
    } else {
      let yCursor = bBottom;
      for (const t of territories) {
        const v = im.get(t) ?? 0;
        if (v <= 0) continue;
        const h = (v / total) * bHeight;
        yCursor -= h;
        parts.push(`<rect class="terr-bar" data-insurer="${s.insurerId}" data-territory="${t}" x="${xPx.toFixed(2)}" y="${yCursor.toFixed(2)}" width="${barW.toFixed(2)}" height="${h.toFixed(2)}" fill="${territoryColour.get(t)}" />`);
      }
    }
    // Insurer label under bar.
    const cx = xPx + barW / 2;
    const label = s.isEntrant ? `*${s.insurerId}` : `${s.insurerId}`;
    parts.push(`<text class="axis-text" x="${cx.toFixed(2)}" y="${(bBottom + 14).toFixed(2)}" text-anchor="middle">${label}</text>`);
  }

  // Territory legend.
  let lx = left;
  const ly = bTop - 4;
  for (const t of territories) {
    parts.push(`<rect x="${lx.toFixed(2)}" y="${(ly - 8).toFixed(2)}" width="9" height="9" fill="${territoryColour.get(t)}" />`);
    parts.push(`<text class="legend-text" x="${(lx + 12).toFixed(2)}" y="${ly.toFixed(2)}">${t}</text>`);
    lx += 12 + t.length * 6.2 + 14;
  }

  parts.push(`</svg>`);
  return parts.join("");
}
