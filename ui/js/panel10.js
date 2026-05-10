// Panel 10: Phenomenon Robustness (multi-run scorecard).
//
// Reduces an array of databases (one per seed) to a per-phenomenon detection
// rate, e.g. "Underwriting cycle detected in 87 / 100 runs (87%)".
//
//   prepPanel10Data(dbs, opts) → { runCount, phenomena: [{ id, label, detected, total, ratio, threshold }, ...] }
//   renderPanel10(data, opts)  → DOM element (or string with `asString: true`)
//
// Detectors operate on analysis-only year statistics (warmup excluded) and
// return a boolean per database.

const CYCLE_THRESHOLD_PP = 0.02; // 2 percentage points

function analysisStats(db) {
  const warmup = db.getWarmupYears();
  return db.getYearStats().filter((s) => s.year > warmup);
}

export function detectCycle(db) {
  const stats = analysisStats(db);
  let lo = Infinity, hi = -Infinity;
  for (const s of stats) {
    if (!s.sum_insured) continue;
    const rol = s.bound_premium / s.sum_insured;
    if (rol < lo) lo = rol;
    if (rol > hi) hi = rol;
  }
  if (!isFinite(lo) || !isFinite(hi)) return false;
  return (hi - lo) > CYCLE_THRESHOLD_PP;
}

export function detectInsolvencies(db) {
  for (const s of analysisStats(db)) {
    if ((s.insolvencies ?? 0) > 0) return true;
  }
  return false;
}

export function detectMarketEntry(db) {
  // "Initial" insurers are admitted at day 0 alongside SimulationStart;
  // post-warmup entry is any InsurerEntered after day 0.
  for (const e of db.events) {
    if (e.type === "InsurerEntered" && e.day > 0) return true;
  }
  return false;
}

export function detectCatCrisis(db) {
  // A year where at least one cat event coincides with at least one insolvency.
  // This is a conservative proxy for the "shared occurrence depletes multiple
  // insurers" phenomenon — the panel is a rough scorecard, not an exact test.
  for (const s of analysisStats(db)) {
    if ((s.cat_event_count ?? 0) >= 2 && (s.insolvencies ?? 0) > 0) return true;
  }
  return false;
}

export const PHENOMENA = [
  { id: "cycle",         label: "Underwriting cycle",            detail: "rate-on-line range across analysis years > 2pp", detect: detectCycle },
  { id: "cat_crisis",    label: "Catastrophe-amplified crisis",  detail: "year with ≥2 cat events AND ≥1 insolvency",      detect: detectCatCrisis },
  { id: "insolvencies",  label: "Insolvencies observed",         detail: "≥1 InsurerInsolvent in analysis years",          detect: detectInsolvencies },
  { id: "market_entry",  label: "Post-warmup market entry",      detail: "≥1 InsurerEntered in analysis years",            detect: detectMarketEntry },
];

export function prepPanel10Data(dbs, _opts = {}) {
  const total = dbs.length;
  const phenomena = PHENOMENA.map((p) => {
    let detected = 0;
    for (const db of dbs) {
      if (p.detect(db)) detected += 1;
    }
    return {
      id: p.id,
      label: p.label,
      detail: p.detail,
      detected,
      total,
      ratio: total === 0 ? 0 : detected / total,
    };
  });
  return { runCount: total, phenomena };
}

// ── Rendering ─────────────────────────────────────────────────────────────

const STYLE = `
.p10-wrap { font: 12px -apple-system, system-ui, sans-serif; font-variant-numeric: tabular-nums; }
.p10-summary { color: var(--fg-dim); font-size: .8em; margin-bottom: .5rem; }
.p10-empty { color: var(--fg-dim); font-style: italic; padding: 1rem; text-align: center; }
.p10-grid { display: flex; flex-direction: column; gap: .35rem; }
.p10-row { display: grid; grid-template-columns: 1fr auto auto; gap: .6rem; align-items: center; padding: .35rem .5rem; border: 1px solid var(--panel-border); border-radius: 3px; background: rgba(255,255,255,.01); }
.p10-row.detected-high { border-color: var(--accent-dim); }
.p10-row.detected-mid  { border-color: #c9a23a; }
.p10-row.detected-low  { border-color: var(--warn); }
.p10-label { color: var(--fg); font-weight: 600; }
.p10-detail { color: var(--fg-dim); font-size: .8em; font-weight: 400; margin-top: .1em; display: block; }
.p10-count { color: var(--fg-dim); font-size: .85em; }
.p10-ratio { color: var(--accent); font-weight: 600; min-width: 3.5em; text-align: right; }
.p10-row.detected-mid .p10-ratio { color: #c9a23a; }
.p10-row.detected-low .p10-ratio { color: var(--warn); }
.p10-bar { grid-column: 1 / -1; height: 4px; background: var(--panel-border); border-radius: 2px; overflow: hidden; margin-top: .25rem; }
.p10-bar > span { display: block; height: 100%; background: var(--accent); }
.p10-row.detected-mid .p10-bar > span { background: #c9a23a; }
.p10-row.detected-low .p10-bar > span { background: var(--warn); }
`;

export function renderPanel10(data, opts = {}) {
  const asString = opts.asString === true;
  const html = buildHTML(data);
  if (asString) return html;
  const wrap = document.createElement("div");
  wrap.className = "p10-wrap";
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
    return `<div class="p10-empty">load multiple <code>events_seed_*.ndjson</code> files to populate the scorecard</div>`;
  }
  const summary = `<div class="p10-summary">phenomenon detection across ${data.runCount} runs</div>`;
  const rows = data.phenomena.map(rowHTML).join("");
  return summary + `<div class="p10-grid">${rows}</div>`;
}

function rowHTML(p) {
  const pct = (p.ratio * 100);
  const bucket = pct >= 75 ? "detected-high" : pct >= 25 ? "detected-mid" : "detected-low";
  const pctStr = `${pct.toFixed(0)}%`;
  return `<div class="p10-row ${bucket}">`
    + `<div><span class="p10-label">${escapeHtml(p.label)}</span><span class="p10-detail">${escapeHtml(p.detail)}</span></div>`
    + `<div class="p10-count">${p.detected} / ${p.total}</div>`
    + `<div class="p10-ratio">${pctStr}</div>`
    + `<div class="p10-bar"><span style="width:${pct.toFixed(1)}%"></span></div>`
    + `</div>`;
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]
  ));
}
