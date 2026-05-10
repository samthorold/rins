// Panel 7: Year Character Table.
//
// Sortable, filterable HTML table replicating the terminal year character
// table. Rows are clickable to broadcast a year-selection across panels.
//
//   prepPanel7Data(db, opts) → { rows, warmupYears, expenseRatio }
//   renderPanel7(data, opts) → HTMLDivElement (or string when `asString: true`)
//
// Each row carries the columns from `docs/ui.md` Panel 7. CR EWMA matches
// `src/main.rs` (alpha = 1/3); `ap_tp` is `1 + clamp(cr_ewma - 1, -0.10, 0.80)`.

import { ewmaSeries } from "./panel1.js";

const DEFAULT_EXPENSE_RATIO = 0.344;
const DEFAULT_EWMA_ALPHA = 1 / 3;

// Columns: [key, label, format, numeric?]
const COLUMNS = [
  ["year",          "Year",       (v) => String(v),                     true],
  ["assets_b",      "Assets(B)",  (v) => v == null ? "—" : v.toFixed(2), true],
  ["gul_b",         "GUL(B)",     (v) => v == null ? "—" : v.toFixed(2), true],
  ["cat_gul_pct",   "CatGUL%",    (v) => v == null ? "—" : v.toFixed(1), true],
  ["coverage_b",    "Coverage(B)",(v) => v == null ? "—" : v.toFixed(2), true],
  ["claims_b",      "Claims(B)",  (v) => v == null ? "—" : v.toFixed(2), true],
  ["loss_ratio",    "LossR%",     (v) => v == null ? "—" : (v * 100).toFixed(1), true],
  ["combined_ratio","CombR%",     (v) => v == null ? "—" : (v * 100).toFixed(1), true],
  ["cr_ewma",       "CrEwma%",    (v) => v == null ? "—" : (v * 100).toFixed(1), true],
  ["rate_on_line",  "Rate%",      (v) => v == null ? "—" : (v * 100).toFixed(2), true],
  ["cat_event_count","Cats#",     (v) => String(v ?? 0),                 true],
  ["total_capital_b","TotalCap(B)",(v) => v == null ? "—" : v.toFixed(2),true],
  ["dropped",       "Dropped#",   (v) => String(v ?? 0),                 true],
  ["ap_tp",         "ApTp",       (v) => v == null ? "—" : v.toFixed(2), true],
  ["insurer_count", "Insurers",   (_, row) => formatInsurerCell(row),    true],
  ["gini",          "Gini",       (v) => v == null ? "—" : v.toFixed(3), true],
  ["cr_sens_mean",  "CrSens",     (v) => v == null ? "—" : v.toFixed(2), true],
  ["cap_sens_mean", "CapSens",    (v) => v == null ? "—" : v.toFixed(2), true],
];

const CENTS_PER_BUSD = 100_000_000_000;

export function prepPanel7Data(db, opts = {}) {
  const expenseRatio = opts.expenseRatio ?? DEFAULT_EXPENSE_RATIO;
  const ewmaAlpha = opts.ewmaAlpha ?? DEFAULT_EWMA_ALPHA;
  const includeWarmup = opts.includeWarmup ?? false;
  const warmupYears = db.getWarmupYears();
  const stats = db.getYearStats();
  const filtered = includeWarmup ? stats : stats.filter((s) => s.year > warmupYears);

  const crValues = filtered.map((s) =>
    s.bound_premium > 0 ? s.claims / s.bound_premium + expenseRatio : null,
  );
  const ewma = ewmaSeries(crValues, ewmaAlpha);

  const rows = filtered.map((s, i) => {
    const total_gul = (s.attr_gul ?? 0) + (s.cat_gul ?? 0);
    const loss_ratio = s.bound_premium > 0 ? s.claims / s.bound_premium : null;
    const combined_ratio = crValues[i];
    const cr_ewma = ewma[i];
    const rate_on_line = s.sum_insured > 0 ? s.bound_premium / s.sum_insured : null;
    const ap_tp = cr_ewma == null ? null : 1 + clamp(cr_ewma - 1, -0.10, 0.80);
    return {
      year: s.year,
      total_assets: s.total_assets ?? 0,
      assets_b: (s.total_assets ?? 0) / CENTS_PER_BUSD,
      gul: total_gul,
      gul_b: total_gul / CENTS_PER_BUSD,
      cat_gul_pct: total_gul > 0 ? (s.cat_gul / total_gul) * 100 : 0,
      coverage: s.sum_insured,
      coverage_b: s.sum_insured / CENTS_PER_BUSD,
      claims: s.claims,
      claims_b: s.claims / CENTS_PER_BUSD,
      loss_ratio,
      combined_ratio,
      cr_ewma,
      rate_on_line,
      cat_event_count: s.cat_event_count ?? 0,
      total_capital: s.total_capital ?? 0,
      total_capital_b: (s.total_capital ?? 0) / CENTS_PER_BUSD,
      dropped: s.dropped ?? 0,
      ap_tp,
      insurer_count: s.insurer_count ?? 0,
      entrants: s.entrants ?? 0,
      insolvencies: s.insolvencies ?? 0,
      gini: s.gini ?? 0,
      cr_sens_mean: s.cr_sens_mean ?? 0,
      cap_sens_mean: s.cap_sens_mean ?? 0,
    };
  });

  return { rows, warmupYears, expenseRatio, ewmaAlpha };
}

function clamp(v, lo, hi) {
  return Math.max(lo, Math.min(hi, v));
}

export function formatInsurerCell(row) {
  const base = String(row.insurer_count ?? 0);
  const plus = row.entrants ?? 0;
  const minus = row.insolvencies ?? 0;
  if (plus > 0 && minus > 0) return `${base} +${plus}-${minus}`;
  if (plus > 0) return `${base} +${plus}`;
  if (minus > 0) return `${base} -${minus}`;
  return base;
}

export function sortRows(rows, key, dir) {
  const mul = dir === "desc" ? -1 : 1;
  return [...rows].sort((a, b) => {
    const av = a[key], bv = b[key];
    const aNull = av === null || av === undefined || Number.isNaN(av);
    const bNull = bv === null || bv === undefined || Number.isNaN(bv);
    if (aNull && bNull) return 0;
    if (aNull) return 1;   // nulls always last
    if (bNull) return -1;
    if (av < bv) return -1 * mul;
    if (av > bv) return 1 * mul;
    return 0;
  });
}

// ---------- Rendering ----------

const STYLE = `
.p7-wrap { width: 100%; overflow-x: auto; font-variant-numeric: tabular-nums; }
.p7-controls { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; margin-bottom: 0.5rem; font-size: 0.85em; color: var(--fg-dim); }
.p7-controls input { background: var(--bg); color: var(--fg); border: 1px solid var(--panel-border); padding: 0.1rem 0.3rem; border-radius: 3px; width: 4.5rem; font: inherit; }
.p7-controls button { background: transparent; color: var(--fg); border: 1px solid var(--panel-border); padding: 0.1rem 0.4rem; border-radius: 3px; cursor: pointer; font: inherit; }
.p7-controls button:hover { border-color: var(--accent-dim); }
.p7-table { border-collapse: collapse; width: 100%; font-size: 0.8em; }
.p7-table th, .p7-table td { padding: 0.2rem 0.5rem; text-align: right; border-bottom: 1px solid var(--panel-border); white-space: nowrap; }
.p7-table th:first-child, .p7-table td:first-child { text-align: left; }
.p7-table th { color: var(--fg-dim); font-weight: 600; cursor: pointer; user-select: none; position: sticky; top: 0; background: var(--panel); }
.p7-table th:hover { color: var(--fg); }
.p7-table th .p7-arrow { color: var(--accent); margin-left: 0.2em; }
.p7-table tbody tr { cursor: pointer; }
.p7-table tbody tr:hover { background: rgba(255,255,255,0.03); }
.p7-table tbody tr.p7-selected { background: rgba(142,192,124,0.12); }
.p7-empty { color: var(--fg-dim); font-style: italic; padding: 0.5rem; }
`;

export function renderPanel7(data, opts = {}) {
  const asString = opts.asString === true;
  const { rows } = data;

  const state = {
    sortKey: "year",
    sortDir: "asc",
    minYear: null,
    maxYear: null,
    minCats: null,
  };

  if (asString) {
    return buildHTML(rows, state);
  }

  const wrap = document.createElement("div");
  wrap.className = "p7-wrap";

  const styleEl = document.createElement("style");
  styleEl.textContent = STYLE;
  wrap.appendChild(styleEl);

  const controls = document.createElement("div");
  controls.className = "p7-controls";
  controls.innerHTML = `
    <span>filter:</span>
    <label>year ≥ <input type="number" data-filter="minYear" /></label>
    <label>year ≤ <input type="number" data-filter="maxYear" /></label>
    <label>cats# ≥ <input type="number" data-filter="minCats" /></label>
    <button type="button" data-action="reset">reset</button>
  `;
  wrap.appendChild(controls);

  const tableHost = document.createElement("div");
  wrap.appendChild(tableHost);

  let onYearClick = opts.onYearClick;

  function rerender() {
    tableHost.innerHTML = buildHTML(applyFilters(rows, state), state);
    // Wire header clicks for sort.
    tableHost.querySelectorAll("th[data-key]").forEach((th) => {
      th.addEventListener("click", () => {
        const key = th.getAttribute("data-key");
        if (state.sortKey === key) {
          state.sortDir = state.sortDir === "asc" ? "desc" : "asc";
        } else {
          state.sortKey = key;
          state.sortDir = key === "year" ? "asc" : "desc";
        }
        rerender();
      });
    });
    // Wire row clicks.
    tableHost.querySelectorAll("tr[data-year]").forEach((tr) => {
      tr.addEventListener("click", () => {
        const y = Number(tr.getAttribute("data-year"));
        if (onYearClick) onYearClick(y);
      });
    });
  }

  controls.querySelectorAll("input[data-filter]").forEach((inp) => {
    inp.addEventListener("input", () => {
      const key = inp.getAttribute("data-filter");
      const v = inp.valueAsNumber;
      state[key] = Number.isFinite(v) ? v : null;
      rerender();
    });
  });
  controls.querySelector("button[data-action=reset]").addEventListener("click", () => {
    state.minYear = state.maxYear = state.minCats = null;
    controls.querySelectorAll("input[data-filter]").forEach((i) => { i.value = ""; });
    rerender();
  });

  rerender();

  // Public API on the node for the shell to drive selection highlight.
  wrap.setSelectedYear = (year) => {
    tableHost.querySelectorAll("tr[data-year]").forEach((tr) => {
      tr.classList.toggle(
        "p7-selected",
        year != null && Number(tr.getAttribute("data-year")) === year,
      );
    });
  };
  wrap.setOnYearClick = (cb) => { onYearClick = cb; };

  return wrap;
}

function applyFilters(rows, state) {
  return rows.filter((r) => {
    if (state.minYear != null && r.year < state.minYear) return false;
    if (state.maxYear != null && r.year > state.maxYear) return false;
    if (state.minCats != null && (r.cat_event_count ?? 0) < state.minCats) return false;
    return true;
  });
}

function buildHTML(rows, state) {
  const sorted = sortRows(rows, state.sortKey, state.sortDir);
  const arrow = state.sortDir === "asc" ? "▲" : "▼";

  const headers = COLUMNS.map(([key, label]) => {
    const active = key === state.sortKey;
    return `<th data-key="${key}">${label}${active ? `<span class="p7-arrow">${arrow}</span>` : ""}</th>`;
  }).join("");

  if (sorted.length === 0) {
    return `<table class="p7-table"><thead><tr>${headers}</tr></thead></table>
            <div class="p7-empty">no rows match the current filter</div>`;
  }

  const body = sorted.map((row) => {
    const cells = COLUMNS.map(([key, _label, fmt]) => {
      const v = row[key];
      const text = fmt(v, row);
      return `<td>${escapeHtml(text)}</td>`;
    }).join("");
    return `<tr data-year="${row.year}">${cells}</tr>`;
  }).join("");

  return `<table class="p7-table"><thead><tr>${headers}</tr></thead><tbody>${body}</tbody></table>`;
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]
  ));
}
