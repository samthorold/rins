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
| Damage fraction model | `Pareto(scale=0.02, shape=3.0)` | `src/perils.rs` `DamageFractionModel::Pareto { scale, shape }` |
| | | `src/config.rs` `CatConfig { pareto_scale: 0.02, pareto_shape: 3.0 }` |

**Implied statistics:**

- Expected damage fraction = **scale × shape / (shape − 1)** = 0.02 × 3.0 / 2.0 = **3%** per affected policy
- Variance is **finite** (shape = 3.0 > 2) — EWMA burning cost estimator is stable
- Implied expected annual cat loss = 0.5 events/yr × 3% × 5 B TIV = **75 M USD/yr**
- Cat ELF = **1.5%**

> **Note on the formula:** The Pareto expected value is `E[X] = scale × shape / (shape − 1)` for shape > 1 (matching `src/perils.rs`). An earlier draft of this document incorrectly stated `scale / (shape − 1)` and should be disregarded.

**Real-world anchor:**
- **GPD shape parameter** from peaks-over-threshold analysis of US hurricane damage: empirical ξ = **0.66–0.80** (95% CI). The relationship to Pareto shape α is ξ = 1/α, giving equivalent α = **1.25–1.52**. The previous shape=1.5 sat at the empirical boundary; shape=3.0 (ξ=0.33) is a softer tail but buys finite variance, which is necessary for EWMA stability. The tail is still substantially heavier than Gaussian.
- **Return period profile at scale=0.02, shape=3.0:** P(df > 5%) = (0.02/0.05)³ = 0.008 → 1-in-125 yr; P(df > 10%) = (0.02/0.10)³ = 0.001 → 1-in-1000 yr. A 1-in-100 yr loss affects ~4.6% of TIV per event; the 1-in-250 yr touches ~6.3%.
- **Per-insurer capital impact at P99:** 14 policies × 4.6% × 50 M = 32 M USD per insurer on ~1–2 B capital → **1.5–3% capital hit**. At P99.9 (10% damage): 70 M USD → **3.5–7% capital hit**. Consistent with the Lloyd's ian observation (~2–20% capital hit for large syndicates in a major event).
- **Hurricane deductibles:** 1–5% of TIV for standard coastal property. scale=0.02 (2% minimum) aligns with the lower end of this range.

**Prior calibration issue (scale=0.05, shape=1.5):** Three compounding problems. First, E[df]=15% and ELF_cat=7.5% overstated expected annual cat loss by roughly 5×. Second, shape=1.5 gives infinite variance, destabilising EWMA — extreme draws had unbounded influence on the burning cost estimator. Third, scale=0.05 applied uniformly meant every policy sustained ≥5% damage in every event, eliminating partial-loss years; combined with full-limit cover this produced near-total-loss outcomes for the insurer in all cat years.

**Key calibration gap:** Scale still applies uniformly to all policies — every risk in the portfolio takes the same damage fraction. Real events affect only a geographic subset. Until spatial correlation is implemented, the uniform draw understates diversification benefit and overstates the correlation of losses across insurers.

---

## §4b Attritional Frequency and Severity [ACTIVE]

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| Annual claim frequency | 0.2 claims/insured/yr | `src/config.rs` `AttritionalConfig { annual_rate: 0.2 }` |
| Damage fraction model | `LogNormal(mu=−3.5, sigma=1.0)` | `src/config.rs` `AttritionalConfig { mu: -3.5, sigma: 1.0 }` |

**Implied statistics:**

- Expected damage fraction = exp(mu + σ²/2) = exp(−3.5 + 0.5) = exp(−3.0) = **5.0%** per claim
- Expected claim size = 5% × 50 M = **2.5 M USD**
- Expected annual attritional loss per insured = 0.2 × 5.0% = **1.0%** of sum-insured
- Attritional ELF = **1.0%**

**Real-world anchor:**
- Commercial property attritional losses (fire, water damage, mechanical breakdown) at Lloyd's typically contribute an attritional loss ratio of **45–50% of net earned premium**. At the target rate of 4.5% ROL, this implies ELF_att ≈ 0.45 × 4.5% ≈ 2%. The current 1.0% sits below this, weighted toward a well-managed portfolio with low working-loss frequency.
- Claim frequency of 0.2/yr (one attritional claim per 5 years per insured) is consistent with large commercial risks ($50 M class) where deductibles and risk management suppress reported frequency.
- Mean damage of 5% ($2.5 M) on a $50 M building is consistent with a partial fire loss, significant water damage, or equipment failure — credible as the "average attritional event" at this size.
- LogNormal(σ=1.0) is standard for property damage severity; 90th-pct damage = exp(−3.5 + 1.28) ≈ 11%, 99th-pct ≈ 25% — rare but non-negligible large losses in the tail.

**Prior calibration issue (rate=2.0, mu=−3.0):** annual_rate=2.0 implied 2 claims per insured per year, and E[df]=8.2% per claim gave ELF_att=16.4% — roughly 10–50× realistic commercial property loading. This was the dominant driver of inflated rates (43.5% ROL) and made benign-year combined ratios implausibly low (~70–75%).

**Combined ELF summary:**

| Peril | ELF |
|-------|-----|
| Attritional | 1.0% |
| Cat (WindstormAtlantic) | 1.5% |
| **Total** | **2.5%** |

---

## §5 Pricing / Rate on Line [ACTIVE]

Pricing is actuarial (ATP-based), not a fixed rate. Each insurer maintains a live expected loss fraction (ELF) and derives the Actuarially Technical Price from it each time a quote is requested.

| Parameter | Current value | Code location |
|-----------|--------------|---------------|
| ATP formula | `ELF × sum_insured / target_loss_ratio` | `src/insurer.rs` `actuarial_price()` |
| Initial ELF prior | 0.025 (= 1.0% att + 1.5% cat) | `src/config.rs` `InsurerConfig { expected_loss_fraction: 0.025 }` |
| Target loss ratio | 0.55 | `src/config.rs` `InsurerConfig { target_loss_ratio: 0.55 }` |
| Expense ratio | 0.344 | `src/config.rs` `InsurerConfig { expense_ratio: 0.344 }` |
| EWMA credibility | 0.3 | `src/config.rs` `InsurerConfig { ewma_credibility: 0.3 }` |
| ELF update | `α × realized_burning_cost + (1−α) × old_ELF` each YearEnd | `src/insurer.rs` `on_year_end()` |

**Implied premium at initial prior:** ATP = 0.025 / 0.55 × 50 M = **2.27 M USD per policy** → **rate on line ≈ 4.5%**.

**Rate dynamics:** the EWMA ties the ATP to observed burning cost. In benign years (no cat), realized LF ≈ ELF_att ≈ 1.0%; ELF drifts down toward 1.0%, softening the rate. A cat year spikes realized LF and hardens ELF. This produces a nascent soft/hard cycle without any explicit cycle-signalling mechanism. Current `ewma_credibility=0.3` (α): medium responsiveness — ELF decays ~70% of the way from prior to realized in ~4–5 years of steady experience.

**Real-world anchor — market RoL ranges:**
- US commercial property direct: **1–5% RoL** in a normal market; 3–8% in a hard market post-major cat
- Lloyd's direct property specialist (pre-Ian): **2–4% RoL**; post-Ian renewals rose 30–60%
- Current rate ≈ 4.5% sits in the upper-normal to moderate-hard range — appropriate for a portfolio with no attachment and full-limit coverage

**Real-world anchor — Lloyd's combined ratios:**
- Lloyd's 2024: combined ratio **86.9%** (attritional LR 47.1%, major claims 7.8%, expense ratio 34.4%)
- Lloyd's 2023: combined ratio **84.0%** (attritional LR 48.3%)
- Property reinsurance segment 2024: combined ratio **75%** (benefiting from prior-year Ian/Ida reserve releases)
- Outlook (2025–2026): 90–95% as market softens

**Implied economics at initial ELF=0.025, target LR=0.55:**

| Year type | Expected LR | Expected CombR |
|-----------|------------|----------------|
| Benign (attritional only) | ELF_att / (ELF/target_LR) = 1.0% / 4.55% = **22%** | **56%** |
| Average cat year | ELF_total / rate = 2.5% / 4.55% = **55%** | **89%** |
| Severe cat (2× mean damage) | ~110% | **144%** |

Benign years are structurally very profitable (56% CombR) because the premium contains a cat loading (~1.5%/4.55% = 33 percentage points of rate) that goes unclaimed. This is correct: pooling reserves the cat loading for rare bad years, and the long-run average converges to the 89% target. Lloyd's own results show the same bimodality: 75–84% in good years, 110–120% in major cat years.

The target combined ratio of **89.4%** (= 55% LR + 34.4% expenses) aligns with Lloyd's 2023–2024 actuals and the market's stated 90–95% outlook.

The current rate is **adaptive but not fully competitive**: the underwriter channel always sets `premium = ATP` (zero margin loading), so no insurer loads above technical price. Competitive dynamics (`[PLANNED]`) will require explicit underwriter margin logic.

---

## §6 Validation Targets [PARTIAL]

The following metrics should be checked after a canonical 5-year run (`cargo run`, then `python3 scripts/analyse_sim.py`).

| Metric | Target range | Basis | Status |
|--------|-------------|-------|--------|
| Annual expected cat loss / TIV | 1–2% | ELF_cat=1.5%; 0.5 events/yr × 3% E[df] × 5B TIV = 75M/yr = 1.5% of TIV | `[ACTIVE]` — computable from events.ndjson |
| Cat loss year std dev / mean | >1 | Pareto(shape=3.0) is still heavy-tailed; cat years should show high year-to-year variance | `[ACTIVE]` — computable from events.ndjson |
| Premium / TIV (effective RoL) | 2–6% | US commercial property direct; EWMA-driven; initial ≈ 4.5% | `[ACTIVE]` — verify from events.ndjson |
| Attritional loss ratio | 20–30% of premium | ELF_att/rate = 1.0%/4.5% ≈ 22% in benign years; Lloyd's attritional LR 47% reflects a richer exposure base | `[ACTIVE]` — computable from events.ndjson |
| Long-run average combined ratio | 85–95% | Lloyd's 84.0% (2023), 86.9% (2024); target_LR=0.55 + expense=0.344 → 89.4% | `[ACTIVE]` — computable from analyse_sim.py over full run |
| Benign-year combined ratio | 50–70% | Cat loading retained in good years; structurally correct even if below Lloyd's actuals | `[ACTIVE]` — computable from events.ndjson |
| Cat-year combined ratio | 100–160% | Severe single-event years; should occasionally exceed 100% without always doing so | `[ACTIVE]` — computable from events.ndjson |
| Insurer capital surviving 20 yr | >0 (all) | No insolvencies expected at calibrated ELF; capital grows in benign years | `[ACTIVE]` — visible in final capitals |
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
