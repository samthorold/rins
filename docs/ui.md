# Event Log UI

Design document for the rins simulation viewer. Captures scope decisions, architecture, and panel specifications for implementation.

---

## Design Decisions

### Primary purpose

The UI exists to provide **visual proof that macro phenomena emerge from micro agent rules**. It is an argument, not a dashboard. The primary audience is someone who needs to be convinced that phenomenon X is real, not hardcoded, and structurally sound.

### Scope (ordered by priority)

| Layer | Priority | Content | Live? |
|-------|----------|---------|-------|
| Phenomena panels | **Core** | Purpose-built chart per phenomenon | No — post-hoc |
| Year character table | **Core** | Existing table as sortable/filterable grid | No |
| Invariant status | **Core** | 19 checks, green/red, drill-down | No |
| Event explorer | Useful | Filterable list with causal chain linking | No |
| Agent inspector | Useful | Per-agent state trajectories | No |
| Multi-run distributions | Useful | Percentile fans across N seeds | No |
| Year-by-year stepping | Nice | Run one year, inspect, continue | Semi-live |
| Progress indicator | Nice | Year counter + event count for long runs | Live (SSE) |
| Mid-run parameter changes | Defer | Breaks reproducibility, unclear value | N/A |

### Technology choice

**Web-based post-hoc viewer.** A Rust binary serves `events.ndjson` and a single HTML page. Client-side JS (Observable Plot or D3) renders charts. No streaming required for the core layer.

Rationale: the simulation finishes in <2s for medium scenarios. Streaming raw events to a live UI is a presentation trick, not a functional necessity. The one exception — very long runs (200+ years) — is served by a simple progress SSE endpoint, not per-event streaming.

### Streaming architecture (deferred)

If live feedback is needed later, the minimal change is an `EventSink` trait:

```rust
pub trait EventSink {
    fn emit(&mut self, event: &SimEvent);
    fn flush(&mut self) {}
}
```

The simulation calls `sink.emit(&event)` after logging. Implementations: `FileSink` (current NDJSON writer), `ChannelSink` (mpsc to TUI/web), `CallbackSink` (fn ptr for embedders). This decouples the sim from its output and enables any future UI without architectural disruption.

### What NOT to stream

Per-event streaming to a browser is impractical at 87K events/sec (large scenario). Instead, stream **digested summaries**:

| Tier | Content | Rate |
|------|---------|------|
| Heartbeat | Year number, event count, wall clock | Per year-end |
| Year summary | `YearStats` struct | Per year-end |
| Filtered events | User-selected types only | On demand |
| Full firehose | Every `SimEvent` | Post-hoc file only |

---

## Panel Specifications

Each panel corresponds to a target phenomenon from `docs/phenomena.md`. Panels are self-contained: given `events.ndjson`, each can be rendered independently. The data extraction for each panel is specified as a recipe over the event stream.

---

### Panel 1: Underwriting Cycle

**Phenomenon:** #1 (Hard/Soft Market Alternation)

**Purpose:** Show rate oscillation driven by catastrophe shocks and capital dynamics — not by a hardcoded cycle.

**Layout:** Single time-series chart, x-axis = simulation year.

**Traces:**

| Trace | Source | Y-axis | Style |
|-------|--------|--------|-------|
| Rate on Line (%) | `LeadQuoteIssued.premium / PolicyBound.sum_insured`, mean per year | Left | Primary line, bold |
| Combined Ratio (%) | `(claims / bound_premium) + expense_ratio`, per year | Left | Secondary line |
| CR EWMA (%) | Exponential moving average of CR (alpha=1/3) | Left | Dashed line |
| Total Capital (B USD) | Sum of last `ClaimSettled.remaining_capital` per insurer at year-end | Right | Area fill, muted |

**Annotations:**

- Vertical bands on years where `cat_event_count >= 2` (double-cat years), labelled "Cat×N"
- Diamond markers on years with `InsurerEntered` events, labelled "+N"
- X markers on years with `InsurerInsolvent` events
- Horizontal reference line at CR = 100% (breakeven)

**Data extraction:**

```
for each year Y (skip warmup):
  rate_on_line = sum(PolicyBound.premium) / sum(PolicyBound.sum_insured)
  loss_ratio   = sum(ClaimSettled.amount) / sum(PolicyBound.premium)
  combined_ratio = loss_ratio + EXPENSE_RATIO
  total_capital = sum of last remaining_capital per insurer_id from ClaimSettled events in year Y
                  (or initial_capital if no claims that year)
  cat_count    = count(LossEvent where peril == WindstormAtlantic) in year Y
  entrants     = count(InsurerEntered) in year Y
  insolvents   = count(InsurerInsolvent) in year Y
```

**What to look for:** Rate spikes 1-2 years after high-loss years. Soft floor binding during benign stretches. Capital entry following hard market signal. The cycle should be visible as an oscillation, not a trend.

---

### Panel 2: Risk Pooling

**Phenomenon:** #0 (Law of Large Numbers)

**Purpose:** Show that individual insured loss volatility compresses to stable aggregate behaviour for attritional losses, but NOT for catastrophe losses.

**Layout:** Two sub-panels side by side.

**Sub-panel A: Attritional pooling**

A box/violin plot or fan chart:
- X-axis: year
- Y-axis: attritional GUL per insured (cents)
- Show individual insured spread (whiskers/band) vs. market mean (bold line)
- Annotate the CV ratio (individual CV / aggregate CV) — target ~√N

**Sub-panel B: Catastrophe pooling (or lack thereof)**

Same layout but for cat GUL in cat-active years only:
- Individual insured cat GUL spread vs. market mean
- CV ratio should be much lower (~2-3×), showing pooling fails for correlated losses

**Data extraction:**

```
for each year Y:
  per_insured_attr_gul[insured_id] = sum(AssetDamage.ground_up_loss where peril == Attritional) for that insured in year Y
  market_mean_attr = mean(per_insured_attr_gul.values())
  individual_cv = std(per_insured_attr_gul.values()) / market_mean_attr
  aggregate_cv = std(yearly market_mean_attr across years) / mean(yearly market_mean_attr)

  # Cat: only in years where LossEvent(WindstormAtlantic) fired
  per_insured_cat_gul[insured_id] = sum(AssetDamage.ground_up_loss where peril == WindstormAtlantic)
  # Same CV calculation
```

**What to look for:** Attritional CV ratio close to √N (≈10 for 100 insureds). Cat CV ratio much lower (2-3×). The contrast proves that pooling works for independent losses and fails for correlated ones.

---

### Panel 3: Capital Crisis Waterfall

**Phenomenon:** #2 (Catastrophe-Amplified Capital Crisis)

**Purpose:** Show how a shared catastrophe occurrence simultaneously depletes multiple insurers, creating a systemic capacity shock.

**Layout:** Stacked area chart + event markers.

**Traces:**

| Trace | Source | Style |
|-------|--------|-------|
| Per-insurer capital | Last `ClaimSettled.remaining_capital` per insurer per year | Stacked areas, one colour per insurer |
| Insolvency markers | `InsurerInsolvent` events | Red X on the insurer's area going to zero |
| Cat event markers | `LossEvent(WindstormAtlantic)` | Vertical red lines with territory label |

**Data extraction:**

```
for each insurer I, each year Y:
  capital[I][Y] = last ClaimSettled.remaining_capital where insurer_id == I in year Y
                  OR initial_capital if no claims
  if InsurerInsolvent { insurer_id: I } in year Y: capital[I][Y] = 0, mark insolvent

cat_events[Y] = list of (day, territory, damage_fraction) from LossEvent(WindstormAtlantic)
```

**What to look for:** All insurers drop simultaneously in cat years (shared occurrence). Capital recovery takes multiple years. Post-crisis, new entrant areas appear at the top of the stack. The crisis is systemic, not idiosyncratic.

---

### Panel 4: Placement Stickiness & Market Concentration

**Phenomenon:** #5 (Relationship-Driven Placement Stickiness)

**Purpose:** Show that broker routing concentrates business among incumbents, and new entrants must earn their way in.

**Layout:** Two sub-panels.

**Sub-panel A: Market share over time**

- X-axis: year
- Y-axis: bound policy count per insurer (stacked bar or area)
- Colour per insurer. New entrants get a distinct hue family.
- Gini coefficient as an overlaid line (right axis)

**Sub-panel B: Relationship score heatmap** (if data available)

- X-axis: insurer (sorted by entry year)
- Y-axis: year
- Cell intensity: broker relationship score (darker = stronger)
- Shows incumbents accumulating dark cells while new entrants start pale

**Data extraction:**

```
for each year Y:
  policies_bound_by_insurer[I] = count(PolicyBound where insurer_id == I) in year Y
  gini = gini_coefficient(policies_bound_by_insurer.values())

  # Relationship score is not currently in the event stream — would need
  # a new event (RelationshipScoreSnapshot) or derivation from PolicyBound history:
  # score[I] = sum(0.80^(current_year - bind_year) for each PolicyBound with insurer_id == I)
```

**Note:** Relationship scores are internal broker state, not currently logged. Two options: (a) derive from PolicyBound history using the known decay formula (score += 1.0 per bind, ×0.80 per year), or (b) add a `RelationshipScoreSnapshot` event at `YearEnd`. Option (a) is preferred — it requires no simulation changes and exercises the event-sourcing principle.

**What to look for:** Incumbent insurers hold share for years after new entrants arrive. Gini rises during entry waves (score-advantaged incumbents vs. zero-score entrants). New entrants' share grows slowly as their scores accumulate.

---

### Panel 5: Accumulation Risk

**Phenomenon:** #8 (Geographic and Peril Accumulation Risk)

**Purpose:** Show per-insurer concentration of cat exposure relative to limits, and how territory routing creates correlated vulnerability.

**Layout:** Two sub-panels.

**Sub-panel A: Cat aggregate utilisation**

- X-axis: year
- Y-axis: cat_aggregate / max_cat_aggregate (0-100%) per insurer
- One line per insurer, or a band showing min/max/mean across insurers
- Horizontal reference at 100% (breach threshold — quotes declined above this)

**Sub-panel B: Territory exposure distribution**

- For each insurer in a selected year: pie/bar showing sum_insured split by territory
- Contrast: diversified insurer (even split) vs. concentrated (heavy in one territory)

**Data extraction:**

```
# Cat aggregate: derived from PolicyBound events
for each insurer I, accumulate:
  cat_aggregate[I] = sum(PolicyBound.sum_insured) for policies where
    risk covers WindstormAtlantic AND policy is in-force (bound_day <= current_day < bound_day + 360)

# Or use LeadQuoteIssued.cat_exposure_at_quote as a snapshot at quote time

# Territory split: from PolicyBound joined with CoverageRequested.risk.territory
for each insurer I in year Y:
  territory_exposure[I][territory] = sum(PolicyBound.sum_insured) grouped by territory
```

**What to look for:** Insurers approach their cat ceiling in active cat territories. When a cat event strikes, the most concentrated insurers take the largest capital hit (visible by cross-referencing with Panel 3). Diversified insurers survive better.

---

### Panel 6: Price Dispersion

**Phenomenon:** #1 sub-hypothesis (per-insurer pricing heterogeneity from Phase 1)

**Purpose:** Show that insurers quote differently for the same risk based on their individual capital state and loss history.

**Layout:** Single chart.

**Traces:**

- X-axis: year
- Y-axis: coefficient of variation of `LeadQuoteIssued.premium` across insurers within the year
- Secondary trace: spread (max premium - min premium) / mean premium

**Annotations:**

- Mark years where new entrants are quoting (they should be at the cheap end)
- Mark post-cat years (depleted insurers should be at the expensive end)

**Data extraction:**

```
for each year Y:
  premiums = [LeadQuoteIssued.premium for all LeadQuoteIssued in year Y]
  cv = std(premiums) / mean(premiums)
  spread = (max(premiums) - min(premiums)) / mean(premiums)

  # Optional: group by insurer to show per-insurer price trajectories
  mean_premium_by_insurer[I] = mean(LeadQuoteIssued.premium where insurer_id == I) in year Y
```

**What to look for:** CV > 0.05 in post-cat years confirms capital-state pricing is active. New entrants (flush capital) quote lower; depleted incumbents quote higher. Dispersion collapses during calm years as capital states converge.

---

### Panel 7: Year Character Table (interactive)

**Not a phenomenon panel** — a reference view for exploration.

**Layout:** Sortable, filterable table replicating the terminal year character table.

**Columns:** Year, Assets(B), GUL(B), CatGUL%, Coverage(B), Claims(B), LossR%, CombR%, CrEwma%, Rate%, Cats#, TotalCap(B), Dropped#, ApTp, Insurers (with +/- delta), Gini, CrSens, CapSens.

**Interactions:**

- Click a row to highlight that year across all phenomena panels (vertical cursor line)
- Sort by any column
- Filter years by range or by condition (e.g., "show only years with Cats# >= 2")

---

### Panel 8: Invariant Dashboard

**Not a phenomenon panel** — a structural health view.

**Layout:** Grid of 19 check badges (green PASS / red FAIL).

**Sections:**

- Mechanics (7 checks): day-offset chain, loss-before-bound, attritional post-bound, expiry timing, claim-after-expiry, cat-fraction consistency, damage-fraction validity
- Integrity (12 checks): GUL cap, aggregate claim cap, claim-loss matching, claim amount > 0, claim-insurer match, quote-bind completeness, bind-insurer match, no duplicate binds, expiry-bind reference, quote-response pairing (3)

**Interactions:**

- Click a failed badge to expand: shows the specific violation(s) with day, policy_id, and context
- Link violations to the event explorer (jump to the offending event)

---

## Multi-Run Panels (future)

When the viewer supports loading multiple `events_seed_*.ndjson` files from `--output-dir`:

### Panel 9: Distribution Fans

- For each of: Loss Ratio, Combined Ratio, Rate on Line, Total Capital
- X-axis: year, Y-axis: value
- Percentile bands: p5-p95 (lightest), p25-p75 (medium), p50 (bold line)
- Shows whether phenomena are robust across seeds or seed-specific artifacts

### Panel 10: Phenomenon Robustness

- For each CONFIRMED/PARTIAL phenomenon, compute a detection metric across N runs
- Display as a scorecard: "Underwriting cycle detected in 87/100 runs (87%)"
- Metrics: cycle detected = rate-on-line range > 2pp; risk pooling confirmed = attritional CV ratio > 7×; etc.

---

## Implementation Notes

### File structure

```
ui/
  index.html          # Single page, all panels
  js/
    data.js           # NDJSON parser + derived metric computation
    panels/
      cycle.js        # Panel 1
      pooling.js      # Panel 2
      capital.js      # Panel 3
      stickiness.js   # Panel 4
      accumulation.js # Panel 5
      dispersion.js   # Panel 6
      table.js        # Panel 7
      invariants.js   # Panel 8
  serve.rs            # Optional: tiny Rust HTTP server (or just open the HTML file)
```

### Data flow

1. Simulation writes `events.ndjson` (existing behaviour, no changes)
2. Viewer loads the file (fetch or drag-and-drop)
3. Client-side JS parses NDJSON, computes `YearStats` and per-panel metrics
4. Each panel renders from its derived data

### Charting library

Observable Plot (preferred) or D3. Observable Plot is declarative, concise, and handles the time-series + annotation patterns well. D3 is the fallback for custom layouts (heatmap, stacked waterfall).

### Performance budget

- 200K events (medium 10-year run): parse + render in <2s
- 2M events (large 10-year run): parse + render in <10s, or offer year-range filtering
- File sizes: 200K events ≈ 20MB NDJSON; consider client-side streaming parse (line-by-line) for large files
