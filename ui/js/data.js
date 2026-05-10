// Data layer for the rins event-log viewer.
//
// Loads `events.ndjson`, normalises each event into `{ day, year, type, data }`,
// builds per-type and per-insurer indexes, and computes one `YearStats` row per
// simulation year. Panels consume this API; they never parse raw JSON.
//
// Year derivation: `SimulationStart` and `YearStart` events drive the current
// year. Events emitted between `YearStart(y)` and the next `YearStart(y+1)` are
// attributed to year `y` — including any `YearEnd` events.

const YEAR_LENGTH_DAYS = 360;

export function parseNDJSONText(text) {
  const events = [];
  let warmupYears = 0;
  let yearStartDay0 = 0;
  let currentYear = 1;
  let sawYearStart = false;

  // Streaming line scan — avoids one huge `split("\n")` allocation for big files.
  const len = text.length;
  let i = 0;
  while (i < len) {
    let j = text.indexOf("\n", i);
    if (j === -1) j = len;
    if (j > i) {
      // Trim trailing \r if present (CRLF safety).
      const end = text.charCodeAt(j - 1) === 13 ? j - 1 : j;
      if (end > i) {
        const line = text.slice(i, end);
        if (line.length > 0) {
          const parsed = JSON.parse(line);
          const day = parsed.day;
          const evObj = parsed.event;
          // Inner event is either { TypeName: { ... } } or a bare string variant.
          let type, data;
          if (typeof evObj === "string") {
            type = evObj;
            data = {};
          } else {
            type = Object.keys(evObj)[0];
            data = evObj[type];
          }

          if (type === "SimulationStart") {
            warmupYears = data.warmup_years ?? 0;
            currentYear = data.year_start ?? 1;
            yearStartDay0 = day;
          } else if (type === "YearStart") {
            currentYear = data.year;
            yearStartDay0 = day;
            sawYearStart = true;
          }

          // Fallback year derivation if no YearStart seen yet (shouldn't happen
          // in real streams but keeps the parser robust on partial files).
          const year = sawYearStart
            ? currentYear
            : Math.floor(day / YEAR_LENGTH_DAYS) + 1;

          events.push({ day, year, type, data });
        }
      }
    }
    i = j + 1;
  }

  return new Database(events, warmupYears);
}

class Database {
  constructor(events, warmupYears) {
    this.events = events;
    this.warmupYears = warmupYears;
    this.byType = new Map();
    this.byInsurer = new Map();
    for (const e of events) {
      let bucket = this.byType.get(e.type);
      if (!bucket) {
        bucket = [];
        this.byType.set(e.type, bucket);
      }
      bucket.push(e);
      const insurerId = e.data && typeof e.data === "object" ? e.data.insurer_id : undefined;
      if (typeof insurerId === "number") {
        let ib = this.byInsurer.get(insurerId);
        if (!ib) {
          ib = [];
          this.byInsurer.set(insurerId, ib);
        }
        ib.push(e);
      }
    }
    this._yearStats = null;
  }

  getWarmupYears() {
    return this.warmupYears;
  }

  getEventsByType(type, year) {
    const bucket = this.byType.get(type);
    if (!bucket) return [];
    if (year === undefined) return bucket;
    return bucket.filter((e) => e.year === year);
  }

  getEventsByInsurer(insurerId, year) {
    const bucket = this.byInsurer.get(insurerId);
    if (!bucket) return [];
    if (year === undefined) return bucket;
    return bucket.filter((e) => e.year === year);
  }

  getYearStats() {
    if (this._yearStats !== null) return this._yearStats;

    // Discover all years present.
    const years = new Set();
    for (const e of this.events) {
      if (typeof e.year === "number") years.add(e.year);
    }
    const sortedYears = [...years].sort((a, b) => a - b);

    // Initialise per-year accumulators.
    const stats = new Map();
    for (const y of sortedYears) {
      stats.set(y, {
        year: y,
        bound_premium: 0,
        sum_insured: 0,
        claims: 0,
        attr_gul: 0,
        cat_gul: 0,
        total_capital: 0,
        cat_event_count: 0,
        insolvencies: 0,
        entrants: 0,
        dropped: 0,
        gini: 0,
        _policies_by_insurer: new Map(),
      });
    }

    // Track last-known capital per insurer across years; carries forward.
    const lastCapital = new Map();
    let runningCapital = 0;
    const activeInsurers = new Set();

    for (const e of this.events) {
      const s = stats.get(e.year);
      if (!s) continue;
      switch (e.type) {
        case "PolicyBound": {
          s.bound_premium += e.data.premium ?? 0;
          s.sum_insured += e.data.sum_insured ?? 0;
          const panel = e.data.panel ?? [];
          for (const [insurerId] of panel) {
            s._policies_by_insurer.set(
              insurerId,
              (s._policies_by_insurer.get(insurerId) ?? 0) + 1,
            );
          }
          break;
        }
        case "ClaimSettled": {
          s.claims += e.data.amount ?? 0;
          const id = e.data.insurer_id;
          const remaining = e.data.remaining_capital;
          if (typeof id === "number" && typeof remaining === "number") {
            const prev = lastCapital.get(id) ?? 0;
            runningCapital += remaining - prev;
            lastCapital.set(id, remaining);
          }
          break;
        }
        case "AssetDamage": {
          const loss = e.data.ground_up_loss ?? 0;
          if (e.data.peril === "WindstormAtlantic") s.cat_gul += loss;
          else s.attr_gul += loss;
          break;
        }
        case "LossEvent":
          if (e.data.peril === "WindstormAtlantic") s.cat_event_count += 1;
          break;
        case "InsurerEntered": {
          s.entrants += 1;
          const id = e.data.insurer_id;
          const cap = e.data.initial_capital ?? 0;
          if (typeof id === "number") {
            activeInsurers.add(id);
            if (!lastCapital.has(id)) {
              lastCapital.set(id, cap);
              runningCapital += cap;
            }
          }
          break;
        }
        case "InsurerInsolvent": {
          s.insolvencies += 1;
          const id = e.data.insurer_id;
          if (typeof id === "number" && lastCapital.has(id)) {
            runningCapital -= lastCapital.get(id);
            lastCapital.set(id, 0);
            activeInsurers.delete(id);
          }
          break;
        }
        case "SubmissionDropped":
          s.dropped += 1;
          break;
        case "YearEnd": {
          // YearEnd belongs to the closing year; snapshot capital and the
          // active insurer set here so gini reflects market concentration
          // including insurers that bound zero policies.
          s.total_capital = runningCapital;
          s._active_snapshot = new Set(activeInsurers);
          break;
        }
      }
    }

    // For years without a YearEnd seen, snapshot capital at end of stream.
    // Fill any year still at 0 by carrying forward from the previous year.
    let lastSeen = 0;
    for (const y of sortedYears) {
      const s = stats.get(y);
      if (s.total_capital === 0) {
        // If we had a non-zero accumulator after processing this year, take it.
        // Otherwise carry the previous year's value.
        s.total_capital = lastSeen;
      }
      lastSeen = s.total_capital;
      // Build gini distribution: every active insurer at year-end contributes
      // a value (0 if they bound nothing this year).
      const snap = s._active_snapshot ?? activeInsurers;
      const values = [];
      for (const id of snap) {
        values.push(s._policies_by_insurer.get(id) ?? 0);
      }
      // Also include insurers who bound but were not in the active snapshot
      // (e.g., bound earlier in the year then went insolvent).
      for (const [id, count] of s._policies_by_insurer) {
        if (!snap.has(id)) values.push(count);
      }
      s.gini = giniCoefficient(values);
      delete s._policies_by_insurer;
      delete s._active_snapshot;
    }

    // Second pass: for years where total_capital was set by YearEnd, lastSeen
    // already updates correctly. The pass above used lastSeen *before* setting
    // s.total_capital, so years that had a YearEnd still propagate forward.

    this._yearStats = sortedYears.map((y) => stats.get(y));
    return this._yearStats;
  }
}

// Standard Gini coefficient over a non-negative numeric array.
// Returns 0 for empty arrays or arrays summing to zero.
export function giniCoefficient(values) {
  if (!values || values.length === 0) return 0;
  const n = values.length;
  if (n === 1) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  let sum = 0;
  let weighted = 0;
  for (let i = 0; i < n; i++) {
    sum += sorted[i];
    weighted += (i + 1) * sorted[i];
  }
  if (sum === 0) return 0;
  return (2 * weighted) / (n * sum) - (n + 1) / n;
}
