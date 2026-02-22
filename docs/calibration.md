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

**Prior work:** Cabral et al. (2024), *"Exploring the Dynamics of the Specialty Insurance Market Using a Novel Discrete Event Simulation Framework: A Lloyd's of London Case Study"*, JASSS 27(2):7 — an agent-based DES of Lloyd's directly relevant to this project's target phenomena.

---

## §2 Exposure Base [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Number of insureds | 100 | `src/config.rs` `SimulationConfig::canonical()` → `n_insureds` |
| Asset value per insured | 50 M USD (5 000 000 000 cents) | `src/config.rs` `ASSET_VALUE` |
| **Total Insured Value (TIV)** | **5 B USD** | derived |

**Real-world anchor:** Lloyd's US wind TIV is not published directly, but market estimates place specialty-market US wind exposure at roughly $5–10 B USD in premium-equivalent limit for a moderately-sized panel. The 5 B USD TIV at a 35% rate-on-line implies 1.75 B USD premium, which is on the high end for a small panel — consistent with a hard-market pricing environment. All insureds are identical by construction — no size heterogeneity yet.

**Known simplification:** uniform asset size understates concentration risk and suppresses the heavy-right-tail loss distribution that real portfolios exhibit. Heterogeneous sum-insured is `[PLANNED]`.

---

## §3 Peril Frequency [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Poisson rate (market-loss events/year) | 0.5 | `src/config.rs` `CatConfig { annual_frequency: 0.5 }` |

**Real-world anchor:**
- US landfalling hurricanes (all categories): **~1.7/yr** (NOAA 1900–present)
- Major hurricanes (Cat 3+) making US landfall: **~0.6/yr** historically (1970s–80s baseline); the modern era (2000s–2010s) sees ~3–4 major Atlantic hurricanes per season, though not all make landfall
- NOAA NCEI billion-dollar events: **67 hurricane events causing >$1 B insured loss since 1980** = ~1.5/yr; events above a ~$5 B industry threshold: roughly **0.3–0.6/yr** (12 events in 1990s, 12 in 2010s)
- Swiss Re sigma 2025: insured natural catastrophe losses exceeded **$100 B for the fifth consecutive year** (2020–2024); two major US hurricanes in 2024 alone — Helene ($16 B insured) and Milton ($25 B insured)

A rate of 0.5/yr sits in the middle of the 0.3–0.6/yr empirical range for Lloyd's-relevant market-loss events (>$5 B industry threshold). This is a *market-loss event* rate, not a landfall rate — one event triggers losses across the whole panel simultaneously.

**Known simplification:** single market-wide event; no spatial correlation (Gulf vs Atlantic landfalls have different frequency profiles). Territory conditioning is `[PLANNED]`.

---

## §4 Peril Severity [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Damage fraction model | `Pareto(scale=0.05, shape=1.5)` | `src/perils.rs` `DamageFractionModel::Pareto { scale, shape }` |
| | | `src/config.rs` `CatConfig { pareto_scale: 0.05, pareto_shape: 1.5 }` |

**Implied statistics:**

- Expected damage fraction = **scale × shape / (shape − 1)** = 0.05 × 1.5 / 0.5 = **15%** of sum-insured per affected policy
- Variance is **infinite** (shape ≤ 2): the distribution has a very heavy tail, appropriate for exploring crisis dynamics
- Implied expected annual cat loss = 0.5 events/yr × 15% × 5 B TIV = **~375 M USD/yr**

> **Note on the formula:** The Pareto expected value is `E[X] = scale × shape / (shape − 1)` for shape > 1 (matching `src/perils.rs`). An earlier draft of this document incorrectly stated `scale / (shape − 1)` and should be disregarded.

**Real-world anchor:**
- **GPD shape parameter** from peaks-over-threshold analysis of US hurricane damage: empirical ξ = **0.66–0.80** (95% CI). The relationship to Pareto shape α is ξ = 1/α, giving equivalent α = **1.25–1.52**. Current shape=1.5 → ξ=0.67: right at the lower bound of the empirical range — defensible.
- **Real event loss-to-TIV:** Katrina ~52% of exposed insured TIV; Harvey ~19–23% (flood gap depresses insured share); Ian ~35–53%
- **Hurricane deductibles:** 1–5% of TIV for standard coastal property (proxy for scale parameter floor; 5% aligns with current scale=0.05)
- A shape of 1.5 produces 1-in-100 year losses several times the mean — plausible for US wind but may overstate 1-in-200 PML used in Lloyd's capital models (typically 25–35% of total limit for peak US wind zones)

**Key calibration gap:** The Pareto minimum (scale=0.05) means *every* policy in an event takes at least 5% damage. Real events affect only a fraction of exposed policies (geographic footprint). Until spatial correlation is implemented, scale doubles as a crude footprint proxy.

---

## §5 Pricing / Rate on Line [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Fixed rate | 0.35 (35% of sum-insured) | `src/config.rs` `InsurerConfig { rate: 0.35 }` |
| Premium calculation | `rate × sum_insured` | `src/insurer.rs` `on_lead_quote_requested` |

**Implied premium:** 35% × 50 M USD = **17.5 M USD per policy per year**.

**Real-world anchor — market RoL ranges:**
- Post-Katrina hard market (2006–2010): **25–50% RoL** for standard cat XL
- Post-Ian renewals (2023–2024): **40–80% RoL** for cat XL; reinsurance rates up **97% cumulatively since 2017** (Swiss Re)
- Current rate=0.35 sits in the post-Katrina hard market range — appropriate for a model that does not yet capture softening dynamics

**Real-world anchor — Lloyd's combined ratios:**
- Lloyd's 2024: combined ratio **86.9%** (attritional LR 47.1%, major claims 7.8%, expense ratio 34.4%)
- Lloyd's 2023: combined ratio **84.0%** (attritional LR 48.3%)
- Property reinsurance segment 2024: combined ratio **75%** (benefiting from prior-year Ian/Ida reserve releases)
- Outlook (2025–2026): 90–95% as market softens

**Implied economics at rate=0.35:**

| Component | Calculation | Result |
|-----------|------------|--------|
| Attritional LR | 2.0 claims/yr × exp(−3.0 + 0.5) × 50 M / 17.5 M | **~47%** |
| Cat LR | 375 M annual cat loss / (100 × 17.5 M premium) | **~21%** |
| Total LR | 47% + 21% | **~68%** |
| Implied CR (+ 34% expense) | 68% + 34% | **~102%** |

The attritional LR of **~47%** almost exactly matches Lloyd's 2024 figure (47.1%) — a strong calibration result. The implied combined ratio of ~102% is slightly above Lloyd's 84–91% range; the gap is expected: no expense model yet, and rate=0.35 reflects a hard market that should soften when cycle dynamics are added.

**Expense loading note:** when the expense model is added, the premium must be loaded multiplicatively — `gross = pure / (1 − expense_ratio)`, not additively. At Lloyd's 2024 expense ratio of 34.4%, the implied break-even loss ratio is 65.6% (= 1 − 0.344); Lloyd's achieved ~52.5% loss ratio in 2024 (86.9% CR − 34.4% expense ratio), generating an 13% underwriting profit margin. A correct expense model will require the rate to rise by roughly 1/(1 − 0.344) ≈ 52% relative to the pure loss rate to match market economics.

The current rate is **fixed** and does not yet reflect:
- Attachment point (currently zero — first-dollar cover)
- Layer width (currently full limit)
- Geographic exposure (currently homogeneous)
- Cycle dynamics (currently fixed, never responds to losses)

Accept as scaffolding until layered contracts and rate-response are implemented (`docs/market-mechanics.md §9`). The rate should decrease when layered contracts introduce realistic attachment points, since attaching above zero dramatically reduces the expected loss ratio.

---

## §6 Validation Targets [PARTIAL]

The following metrics should be checked after a canonical 5-year run (`cargo run`, then `python3 scripts/analyse_sim.py`).

| Metric | Target range | Basis | Status |
|--------|-------------|-------|--------|
| Annual expected cat loss / TIV | 0.5–3% | Corrected E[df]=15%; 0.5 events/yr × 15% = 7.5% on expectation before footprint discount | `[ACTIVE]` — computable from events.ndjson |
| Cat loss year std dev / mean | >1 | Pareto tail (infinite variance); should be highly volatile year-to-year | `[ACTIVE]` — computable from events.ndjson |
| Premium / TIV (effective RoL) | 25–80% | Post-Katrina 25–50%; post-Ian 40–80%; current model 35% | `[ACTIVE]` — currently fixed at 35% |
| Attritional loss ratio | 45–50% | Lloyd's 2023–2024: 48.3% / 47.1% | `[ACTIVE]` — implied ~47% at rate=0.35; verify from events.ndjson |
| Expense ratio (acquisition + management) | ~34% of NEP | Lloyd's 2024: 22.6% acquisition + 11.8% management = 34.4% | `[PLANNED]` — no expense model yet |
| Lloyd's-equivalent combined ratio | 84–95% | Lloyd's 84.0% (2023), 86.9% (2024); softening toward 90–95% in 2025–26 | `[PLANNED]` — requires expense model |
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
| No expense or brokerage model | Premiums are treated as fully retained by syndicates; acquisition costs (~22.6% of NEP) and management expenses (~11.8%) are not deducted. Combined ratio cannot be computed without an expense model. The correct loading formula is `gross = pure / (1 − expense_ratio)`, not additive | `[PLANNED]` — see market-mechanics.md §4.3 |
