# Calibration: US Atlantic Wind / Lloyd's Specialty Market

Parameter values are calibration concerns, not architecture. Architecture is in `docs/market-mechanics.md`. This document records what each parameter is targeting, why the current value was chosen, and what evidence it should eventually satisfy. Validation targets are linked to `docs/phenomena.md`.

**Status badges:** `[ACTIVE]` = implemented and in use; `[PARTIAL]` = parameter present but incompletely specified; `[PLANNED]` = intended but not yet implemented; `[TBD]` = identified gap, approach unresolved.

---

## §1 Scope and Purpose

This document covers **US Atlantic named-storm wind** as modelled by the `WindstormAtlantic` peril, written by a small Lloyd's specialty panel. It does not cover attritional losses, other territories, or other lines of business — those will be added as the simulation grows.

The Lloyd's specialty market is the anchor because:
- It writes catastrophe-exposed US wind on a subscription basis (consistent with the panel/syndicate structure in the simulation)
- Public data (Lloyd's Annual Reports, PRA returns) gives order-of-magnitude validation points
- The underwriting cycle phenomenon (§1 in `docs/phenomena.md`) is well-documented in this market

---

## §2 Exposure Base [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Number of insureds | 100 | `src/config.rs` `SimulationConfig::canonical()` → `n_insureds` |
| Asset value per insured | 50 M USD (5 000 000 000 cents) | `src/config.rs` `ASSET_VALUE` |
| **Total Insured Value (TIV)** | **5 B USD** | derived |

**Real-world anchor:** Lloyd's US wind TIV is not published directly, but market estimates place specialty-market US wind exposure at roughly $5–10 B USD in premium-equivalent limit for a moderately-sized panel. The 5 B USD TIV at a 10% rate-on-line implies 500 M USD premium, which is the right order of magnitude for a small panel. All insureds are identical by construction — no size heterogeneity yet.

**Known simplification:** uniform asset size understates concentration risk and suppresses the heavy-right-tail loss distribution that real portfolios exhibit. Heterogeneous sum-insured is `[PLANNED]`.

---

## §3 Peril Frequency [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Poisson rate (market-loss events/year) | 0.5 | `src/config.rs` `CatConfig { annual_frequency: 0.5 }` |

**Real-world anchor:**
- US landfalling hurricane rate: ~6/yr (NOAA 1851–present all intensities)
- Major hurricanes (Cat 3+) making US landfall: ~0.6/yr historically
- Events causing US industry insured loss >$5 B (Lloyd's-relevant threshold): roughly 0.3–0.6/yr based on AIR/RMS industry figures 1990–2020

A rate of 0.5/yr sits in the middle of the plausible range for meaningful cat events. This is a *market-loss event* rate, not a landfall rate — one event triggers losses across the whole panel simultaneously.

**Known simplification:** single market-wide event; no spatial correlation (Gulf vs Atlantic landfalls have different frequency profiles). Territory conditioning is `[PLANNED]`.

---

## §4 Peril Severity [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Damage fraction model | `Pareto(scale=0.05, shape=1.5)` | `src/perils.rs` `DamageFractionModel::Pareto { scale, shape }` |
| | | `src/config.rs` `CatConfig { pareto_scale: 0.05, pareto_shape: 1.5 }` |

**Implied statistics:**

- Expected damage fraction = scale / (shape − 1) = 0.05 / 0.5 = **10%** of sum-insured per affected policy
- Variance is **infinite** (shape ≤ 2): the distribution has a very heavy tail, appropriate for exploring crisis dynamics
- Implied expected annual cat loss = 0.5 events/yr × 10% × 5 B TIV = **~250 M USD/yr**

**Real-world anchor:**
- Lloyd's US wind combined loss ratio (cat component): roughly 20–30% of premium in a bad year; with a 10% RoL this maps to ~2–3% of TIV, consistent with 250 M / 5 B = 5% on expectation (slightly elevated)
- A shape of 1.5 produces 1-in-100 year losses several times the mean — plausible for US wind but may overstate 1-in-200 PML used in Lloyd's capital models (typically 25–35% of total limit for peak US wind zones)

**Key calibration gap:** The Pareto minimum (scale=0.05) means *every* policy in an event takes at least 5% damage. Real events affect only a fraction of exposed policies (geographic footprint). Until spatial correlation is implemented, scale doubles as a crude footprint proxy.

---

## §5 Pricing / Rate on Line [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Fixed rate | 0.1 (10% of sum-insured) | `src/config.rs` `InsurerConfig { rate: 0.1 }` |
| Premium calculation | `rate × sum_insured` | `src/insurer.rs` `on_lead_quote_requested` |

**Implied premium:** 10% × 50 M USD = **5 M USD per policy per year**.

**Real-world anchor:**
- Catastrophe XL Rate on Line for US wind: typically **3–15%** depending on attachment point and layer exhaustion probability
- Risk-adjusted commercial rate for full-value (no excess layer) US wind: ~5–8% on exposed limit for peak zones; lower for inland / partial exposure

The current rate is a **placeholder** that does not yet reflect:
- Attachment point (currently zero — first-dollar cover)
- Layer width (currently full limit)
- Geographic exposure (currently homogeneous)
- Cycle dynamics (currently fixed, never responds to losses)

Accept as scaffolding until layered contracts are implemented (`docs/market-mechanics.md §9`). The rate should decrease when layered contracts introduce realistic attachment points, since attaching above zero dramatically reduces the expected loss ratio.

---

## §6 Validation Targets [PARTIAL]

The following metrics should be checked after a canonical 5-year run (`cargo run`, then `python3 scripts/analyse_sim.py`).

| Metric | Target range | Basis | Status |
|--------|-------------|-------|--------|
| Annual expected cat loss / TIV | 0.5–3% | Historical US wind industry loss / estimated specialty TIV | `[ACTIVE]` — computable from events.ndjson |
| Cat loss year std dev / mean | >1 | Pareto tail; should be highly volatile year-to-year | `[ACTIVE]` — computable from events.ndjson |
| Premium / TIV (effective RoL) | 3–15% | Lloyd's specialty market rate surveys | `[ACTIVE]` — currently fixed at 10% |
| Insurer capital surviving 5 yr | >0 (all) | Solvency floor not yet enforced; check manually | `[ACTIVE]` — visible in final YearEnd events |
| Renewal retention rate | ~90–95% | Specialty market broker relationships | `[PLANNED]` — no lapse model yet |

**§0 Risk Pooling** in `docs/phenomena.md` is already `[EMERGING]` and provides the first quantitative check. Cat loss volatility is observable but not yet formally tracked in the analysis script.

---

## §7 Known Simplifications [PLANNED / TBD]

The following real-world features are absent from the current model. They are listed here so calibration decisions are made with awareness of what is missing.

| Simplification | Impact on calibration | Status |
|---------------|----------------------|--------|
| Single territory; no Gulf vs Atlantic split | Frequency and severity should differ by territory; blending overstates peak-zone loss for inland risks | `[PLANNED]` |
| No occurrence / aggregate limit distinction | All losses accumulate without limit cap per occurrence; overstates insurer exposure in large events | `[PLANNED]` |
| No reinstatement premiums | Post-cat premium income understated; RoL dynamics muted | `[TBD]` |
| All insureds hit equally in a cat event | No spatial footprint; understates diversification benefit and overstates frequency of total-portfolio events | `[PLANNED]` |
| No demand surge / loss amplification post-cat | Real losses increase 10–20% due to labour/materials scarcity after major events | `[TBD]` |
| Fixed rate; no cycle response | Pricing does not harden after losses; underwriting cycle cannot emerge until rate responds | `[PLANNED]` — target phenomenon §1 |
