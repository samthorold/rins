# Simulation Roadmap

This document captures the phased plan for evolving the simulation's supply and demand mechanics toward full underwriting cycle emergence. Each phase is motivated by a specific structural gap identified in the current model, states a falsifiable hypothesis, and specifies the diagnostic signals that would confirm or deny it.

Phases are ordered by expected impact per implementation cost and by logical dependency. Each phase should be completed with a failing test written first, verified against the invariant suite, and evaluated against the diagnostic criteria before the next phase begins.

---

## Diagnosis: current structural gaps

Phases 1, 3, and 4 are complete. The 200-year canonical run (seed=42, 8×150M insurers at start, 100 insureds, 205 total years, 5yr warmup) reveals the following picture: Rate-on-line oscillates between ~11.5–16.0% RoL, ApTp cycles between 0.90 (soft floor) and 1.45 (post year-124 catastrophe cluster), and 11 years exceed 100% LossR — all cat-driven. Capital entry fires correctly: 7 entrants over 200 years, clustering after severe years. No insolvencies despite a 225% LossR year. Total capital grows monotonically from 1.26B (yr6) to 8.01B (yr205). These are signs of four structural gaps:

**Gap 1 — Coordinator-broadcast pricing.** `[Addressed by Phase 1 — DONE]` Per-insurer capital-state blending is active. Each insurer applies its own `own_ap_tp_factor` weighted by credibility against the market signal. Post-cat rate dispersion (CV 0.07–0.18 by year) confirms competitive pricing is operating.

**Gap 2 — No soft-market supply contraction.** `[Deferred — requires Phase 5 variable line sizes]` Insurers enter when AP/TP > 1.10 but cannot reduce exposure without full exit. Binary exit/re-entry was removed (Phase 2) as unrealistic. Until insurers can express caution by writing a smaller share of each risk rather than exiting entirely, supply contraction is missing from the cycle's lower half. Variable line sizes (Phase 5) are the prerequisite; they make supply contraction continuous rather than binary.

A companion sub-gap: the entry count is uncapped. A sustained 10-year hard market spawns 10 new insurers with no declining marginal attractiveness signal. The real market has a rising supply curve for capital: the nth entrant requires higher expected returns than the (n-1)th. Both the exit floor (Phase 5) and a softer entry signal (market saturation) are needed for a symmetric supply response.

**Gap 3 — Inelastic demand.** `[Addressed by Phase 4 — DONE]` Heterogeneous reservation prices from `LogNormal(ln(0.25), 0.40)` are active. At 14% RoL ~7.5% of insureds price out; at 21% ~33% price out. The `Reject#` column separates demand-constrained from supply-constrained non-placements. Quantity adjustment (limit reduction, deductible increase) is not yet modelled.

**Gap 4 — Bounded exposure, unbounded capital.** `[Requires Phase 5 + Phase 6]` The insured pool is fixed at 100 × 25M USD = 2.5B total exposure. The theoretical maximum single-year GUL is ~1.25B (100 policies, 50% damage cap). TotalCap has grown to 8.01B by year 205: a 225% LossR year (yr124) reduced capital by only 9.5% and produced no insolvencies. The ratio max_GUL / TotalCap converges to zero over time; crisis dynamics become structurally impossible regardless of cat severity. Two mechanisms address this:

- **Variable line sizes (Phase 5):** the link between capital and exposure becomes proportional rather than binary. A larger capital base writes proportionally larger lines and earns proportionally more premium, but also absorbs proportionally larger losses — keeping the loss-to-capital ratio stable as the market grows.
- **Capital distributions (Phase 6):** a payout-ratio mechanism drains surplus each year, preventing monotonic accumulation. Models the Lloyd's Names structure in which profits are distributed annually and capital does not compound indefinitely inside the vehicle.

---

## Phase 1 — Per-insurer capital-state pricing `[DONE — 2026-02-26]`

**Mechanism.** Replace the single coordinator-broadcast `market_ap_tp_factor` with a per-insurer factor that blends the insurer's own capital adequacy and own loss history with the market reference. A capital-depleted insurer prices harder than a well-capitalised new entrant writing the same risk.

The coordinator continues publishing a market reference (current `ap_tp_factor` computation unchanged). Each insurer blends it with an individual capital-depletion signal:

```
capital_depletion  = max(0.0, 1.0 − capital / initial_capital)
cap_depletion_adj  = clamp(capital_depletion × depletion_sensitivity, 0.0, 0.30)
own_cr_signal      = clamp(own_3yr_avg_CR − 1.0, −0.25, 0.40)
own_factor         = clamp(1.0 + own_cr_signal + cap_depletion_adj, 0.90, 1.40)
insurer_ap_tp      = credibility × own_factor + (1 − credibility) × market_factor
```

where `credibility` rises with years of own experience (low in warmup, approaching 1.0 after ~5 years). Each insurer tracks its own rolling 3-year combined ratio independently.

**Primary hypothesis.** After a severe cat year, capital-depleted incumbents hold rates 10–20% above market reference while new entrants (full initial capital, own 3yr CR not yet elevated) price closer to ATP. The effective market-clearing rate — the rate the *cheapest willing insurer* is prepared to offer — is lower than the average incumbent quote, creating genuine competitive erosion rather than administrative softening. Hard market duration increases by at least 2 years in the canonical run.

**Secondary hypotheses.**
- Cross-sectional rate dispersion is measurable: post-cat year, per-insurer premiums for identical risks will differ by 5–15%.
- New entrants gain disproportionate market share in the first 2 years post-entry, then converge toward incumbent rates as their own capital accumulates claims.
- The 3-year rate-collapse pattern (8.05% → 6.23% in 3 years) extends to 5+ years.

**Diagnostics.** Per-insurer `LeadQuoteIssued.premium` visible in `events.ndjson`. Compute the coefficient of variation of quoted premiums for the same sum_insured across insurers, by year. CV > 0.05 post-cat confirms price dispersion. Track which insurer wins business in post-cat years — new entrants should be overrepresented.

**Results (seed=42, 8×150M insurers, 100 insureds, 25yr run).**

Primary hypothesis — *partially confirmed.* The year-16 double-cat (LossR=146.9%) produced a hard market that held for approximately 4 years (rates: 7.65% → 6.65% → 7.17% → 6.67%), compared to the pre-Phase-1 3-year collapse. The target of 5+ years was not reached; capital accumulation remains monotonic (1.22B → 1.89B) and no insolvencies occurred. Gap 2 (voluntary exit) is required to close the soft-market floor.

Secondary hypotheses:
- Rate dispersion **confirmed**: CV of quoted premiums is 0.07–0.18 in every post-warmup year (Year 1 = 0.00 as expected — new entrants, no experience). Dispersion is persistent, not just post-cat.
- New-entrant market share — *not yet measurable* from the year table; requires per-insurer bound-policy counts (Phase 4 diagnostic).
- 5+ year hard-market duration — *not yet reached*; extended from 3 to ~4 years; soft-market supply contraction (variable line sizes, deferred) needed for full confirmation.

**Does not fix.** Demand inelasticity (Gap 3) and supply ratchet (Gap 2). The rate erosion mechanism shifts from administrative to competitive, but there is still no demand-side resistance and no voluntary exit to close the soft-market floor.

---

## Phase 2 — Voluntary exit (soft-market capital withdrawal) `[REMOVED]`

**Rationale for removal.** The binary exit/re-entry mechanism produced unrealistic synchronised behaviour: all insurers sharing similar loss histories hit the runoff CR threshold simultaneously (mass exits in a single year), and all runoff insurers flooded back the moment `market_ap_tp_factor > 1.10` (mass re-entries). Swings like +9/−7 insurers in a single year bear no resemblance to Lloyd's market dynamics.

More fundamentally, binary class exit is the wrong abstraction. Lloyd's syndicates almost never fully withdraw from a class — they reduce participation (line sizes), price themselves out of bad business, or tighten terms. Full class exit carries heavy relational and regulatory costs and is not a lever syndicates use routinely. The intended soft-market withdrawal effect will emerge naturally from variable line sizes and more faithful capital management, which are planned but not yet implemented.

**Deferred.** The mechanism will be revisited once variable participation fractions are implemented (planned). At that point, syndicates can express soft-market caution by reducing their line size rather than exiting entirely, which is the correct market abstraction. See `market-mechanics.md §7.4` for the updated design rationale.

---

## Phase 3 — Relationship-ranked routing `[DONE — 2026-02-27]`

**Mechanism.** Replace round-robin start-index routing with relationship-score–ranked selection. `Broker` accumulates `relationship_scores: HashMap<InsurerId, f64>`: +1.0 per `PolicyBound`, ×0.80 per `YearEnd` (halves in ~3.1 years). `on_coverage_requested` sorts the insurer pool by score descending; cyclic distance from `next_insurer_idx` breaks ties so all-equal scores degenerate to the prior round-robin behaviour. Canonical `quotes_per_submission` changed from `None` (all 8) to `Some(4)` (top-4 by score). Re-entrants retain their decayed score; new InsurerId values start at 0.0.

**Results (seed=42, 8×150M insurers, 100 insureds, 25yr run).**

Primary hypothesis — *partially confirmed.* Gini values in the year table are non-trivial (0.03–0.19), confirming that market share is no longer perfectly uniform. The highest concentration occurs in years 15–16 (Gini=0.114, 0.190) during the entry wave following the year-14 double-cat: incumbents with accumulated scores capture disproportionate share in the first post-crisis year, while the 10+ new entrants start at 0.0 and must compete on price to build scores. In quiet years with stable composition (e.g. years 7–9, Gini=0.035–0.060), concentration is low — incumbents have similar scores from balanced history and k=4-of-7 gives broad coverage. The Gini column serves as the primary diagnostic for all future phases.

**Key patterns:**
- Hard-market/exit years (years 11–12, 14): Gini falls (few surviving insurers, each holding substantial scored relationships with a fraction of the insured pool).
- Entry wave (year 15–16): Gini rises sharply (high-score incumbents + low-score new entrants competing for same k=4 slots).
- Stable soft markets (years 21–25): Gini 0.03–0.12 — low-concentration equilibrium as scores converge.

**Does not fix.** Demand inelasticity (Gap 3). The Gini diagnostic does not yet reveal demand-side dynamics; the `Dropped#` column still conflates supply-constrained and demand-constrained non-placements.

---

## Phase 4 — Demand elasticity (heterogeneous reservation prices) `[ACTIVE]`

**Mechanism.** Each insured draws a `base_max_rate_on_line` at construction from `LogNormal(max_rol_mu, max_rol_sigma)`. Canonical: `LogNormal(ln(0.25), 0.40)` — median reservation price 25% RoL. `Insured::on_quote_presented` compares `premium / sum_insured` against `effective_max_rol() = base_max_rate_on_line + rol_uplift` and emits `QuoteRejected` if the threshold is exceeded. The degenerate case `sigma == 0.0` → every insured gets `exp(mu)` exactly (no RNG consumed; used in tests).

`QuoteRejected` (demand-driven) is distinguished from `SubmissionDropped` (supply-driven) by the `Reject#` column in the year table. The two columns together diagnose whether a capacity crunch is insurer-constrained or price-constrained.

**Observed results (canonical run, 205 years, seed=42).** 46 demand-side rejections over the full run vs thousands of `SubmissionDropped` — demand-side pressure is active but modest at normal rate levels. At year 30 (Rate% ≈ 22.76%, the sharpest hard-market spike in the run), 6 rejections occur in a single year, confirming the elasticity activates precisely when rates breach the central mass of the LogNormal. At normal rates (12–13%), 1–2 spurious rejections per year appear from the left tail (a small number of insureds with base_max_rol ≈ 0.10–0.12 encountering a slightly expensive per-insurer quote). These are real demand behaviour, not artefacts.

**Calibration.** At 14% (hard market): ~7.5% of insureds reject. At 18%: ~21%. At 21%: ~33%. At 6–8% (normal): <2%. The curve is well-separated from the normal operating range.

**What this did not fix.** No quantity adjustment: buyers who do purchase still buy at full `sum_insured` with zero attachment. New capital still competes for the same per-policy premium volume once the marginal buyers exit. Rate collapse in post-cat soft markets remains faster than empirical Lloyd's cycles. The two remaining structural gaps are: (1) programme restructuring (limit/deductible adjustment), and (2) competitive individual pricing replacing the coordinator-broadcast `market_ap_tp_factor`.

---

## Phase 5 — Variable line sizes and panel assembly

**Why this is the next phase.** The 200-year run (Gap 4 above) demonstrated that with fixed line sizes (`line_size = 1`) capital accumulates without bound on a fixed exposure pool. Variable lines are also the prerequisite for soft-market supply contraction (§7.4), post-catastrophe concentration dynamics (phenomenon 7), and the full lead-follow subscription model (Phase 7). They are the single change with the widest downstream unlock.

**Mechanism.** Each insurer's `on_lead_quote_requested` returns a `(premium, line_size)` pair rather than just a premium. `line_size ∈ (0.0, 1.0]` is computed from two factors the insurer already holds:

```
capacity_line = min(net_line_capacity × capital / sum_insured, 1.0)
                                          // capital-linked limit — already computed

pricing_line  = clamp((own_ap_tp_factor - floor_factor) / (1.0 - floor_factor), 0.0, 1.0)
                                          // at floor_factor: write nothing; at 1.0+: write max
                                          // canonical floor_factor ≈ 0.85

line_size = min(capacity_line, pricing_line)
```

`pricing_line` is the key addition: it makes the offered line a continuous function of pricing adequacy. When `own_ap_tp_factor` is high (hard market, adequate rates) the insurer writes its full capital-limited line; when it is close to the floor it writes a small line or declines entirely. This is the mechanism by which soft-market supply contraction emerges from individual insurer behaviour rather than from a binary exit decision.

The broker accumulates lines from solicited insurers until the total reaches 1.0, approaching additional insurers (score-ranked) if the initial panel is undersubscribed. A risk that cannot be fully subscribed after exhausting the insurer pool emits `SubmissionDropped` — the same existing event that already triggers renewal.

**Architectural scope.** This is the largest protocol change in the simulation to date. The changes are self-contained within the placement and settlement layers:

| Component | Change |
|---|---|
| `LeadQuoteIssued` | Add `line_size: f64` field |
| `PolicyBound` | Replace `insurer_id: InsurerId` with `panel: Vec<(InsurerId, f64)>` |
| `ClaimSettled` | Emitted once per panel member; `amount = total_loss × member_line_size` |
| `Insurer::on_lead_quote_requested` | Compute and return `line_size` |
| `Broker::on_lead_quote_issued` | Accumulate panel; emit `QuotePresented` when ≥ 1.0 subscribed |
| `Market::on_asset_damage` | Route claims to all panel members proportionally |
| `Insurer::on_policy_bound/expired` | Apply `sum_insured × line_size` to exposure tracking |

The day-offset invariants are unchanged. The quoting protocol is unchanged from the insured's perspective (one `QuotePresented` → one `QuoteAccepted` → one `PolicyBound`). The insolvency path is unchanged. The panel-splitting infrastructure is already noted in market-mechanics.md §2.1 as scaffolded.

**Invariants requiring update.** Inv 11 (ClaimSettled insurer matches PolicyBound insurer) becomes: every `ClaimSettled.insurer_id` is a member of the `PolicyBound.panel` for that policy. Add: sum of `ClaimSettled.amount` for a (policy, loss) equals total insured loss, within integer rounding.

**Primary hypothesis.** After a cat event, capital-depleted incumbents reduce their offered line size rather than declining outright. The aggregate market line fraction per risk falls, causing more risks to be undersubscribed. `Dropped#` rises *without* insurers exiting. When pricing recovers, line fractions expand and the `Dropped#` falls — a smooth, insurer-specific supply response rather than a step-change. This produces genuine soft-market floor emergence and closes the lower half of the underwriting cycle.

**Diagnostics.** New column: `AvgLine%` — mean offered line fraction across `LeadQuoteIssued` events per year. In benign years (adequate rates, full capital) this should approach `capacity_line` for most insurers (~30%). Post-cat it should fall toward `pricing_line`. Track the cross-correlation of `AvgLine%` and `Dropped#` over multi-year windows — these should move together, confirming that line contraction is the mechanism driving capacity shortage, not refusals.

---

## Phase 6 — Capital distributions

**Why this is needed.** Gap 4 shows that TotalCap grows 6.3× over 200 years while the exposure ceiling stays fixed at 2.5B. Even variable line sizes (Phase 5) slow but do not stop this: each insurer writes proportionally larger lines and earns proportionally more premium, still retaining most profits. Without a drain, the `max_GUL / TotalCap` ratio converges to zero and crises become structurally impossible at century-scale. Variable lines fix the *relative* exposure-to-capital ratio within a year; distributions fix the *long-run* level.

**Mechanism.** At `YearEnd`, each insurer distributes a fraction of its annual underwriting profit:

```
year_underwriting_profit = ytd_premium - ytd_claims - ytd_expenses
distributable = year_underwriting_profit.max(0.0) * payout_ratio    // canonical: 0.70
capital -= distributable
// emit CapitalDistributed { insurer_id, amount }
```

Losses are not called (Names are not compelled to inject capital on a bad year — that is a Lloyd's Names call mechanism, planned separately). Payout applies only in profit years.

**Lloyd's context.** Lloyd's syndicates operate on a 3-year account. When an underwriting year closes, profits crystallise and are distributable to Names. Names choose whether to recommit capital or withdraw. The 70% payout ratio reflects managing agencies retaining 20–30% for solvency buffer and profit commission; Names extract the rest. Historically profitable Lloyd's syndicates have returned capital at payout ratios of 60–80% of underwriting profit in good years.

**Effect on long-run dynamics.** With a 70% payout and typical annual retained earnings of ~5% of capital, the equilibrium capital level stabilises at approximately `initial_capital / (1 - retention_fraction × annual_return_rate)`. The exact level is a calibration outcome, not a designer input. After a severe cat year (no profit → no distribution), capital dips and takes several years to recover, keeping the market in the vulnerability regime where crises bite. After a quiet year, the distribution prevents runaway accumulation. The capital level oscillates around a stable mean rather than drifting upward indefinitely.

**Diagnostic.** New event `CapitalDistributed { insurer_id, amount }`. Add `Distributions(B)` to the year table showing total market capital returned in that year. In a well-calibrated run, TotalCap should plateau rather than grow monotonically: `Distributions(B) ≈ retained_earnings_rate × TotalCap` in benign years. Post-cat years should show near-zero distributions as profits collapse. The capital trajectory should resemble a mean-reverting process with cat-driven excursions downward rather than a monotonic ramp.

---

## Phase 7 — Lead-follow subscription market

*(Previously Phase 5. Variable line sizes, Phase 5, must be operational before this phase is started — follower behaviour is only meaningful when panel subscription is real.)*

**Mechanism.** Full Lloyd's subscription model: broker nominates a lead insurer based on relationship score. Lead quotes in lead mode (no prior quote visible, full individual pricing from Phase 1). Followers observe the lead quote and shade ±Δ based on their own actuarial view and relationship. Panel assembly uses the variable line size machinery from Phase 5; followers can write a smaller line than the lead.

This is the prerequisite for phenomena 3 (Broker-Syndicate Network Herding), 5 (Relationship-Driven Placement Stickiness), and 7 (Post-Catastrophe Market Concentration Surge).

**Primary hypothesis.** Lead syndicates with strong relationship scores set the market price for a risk; followers amplify pricing errors in both directions. Market-wide rate movements are faster in one direction (herding amplifies hardening post-cat) and stickier in the other (relationship stickiness slows softening as established leads hold rates). Cycle asymmetry — faster hardening than softening — matches the empirical record.

---

## Sequencing rationale

Phases are ordered by two criteria: (a) independent value — does the phase produce a testable hypothesis in isolation, or does it only matter in combination with later phases? (b) architectural dependency — does a later phase require the earlier one's infrastructure?

| Phase | Independent value | Unlocks |
|---|---|---|
| 1 — Individual pricing | High — rate dispersion and hard-market duration immediately testable | Phase 3, Phase 7 |
| 2 — Voluntary exit | Removed — see rationale above | — |
| 3 — Competitive quoting | Medium-High — market share concentration and new-entrant undercutting | Phase 7 |
| 4 — Demand elasticity | Medium — cycle modulation and supply/demand separation | Phenomenon 9 |
| 5 — Variable line sizes | **High — prerequisite for all remaining phases; closes soft-market floor** | Phase 6, Phase 7, Phenomena 6, 7 |
| 6 — Capital distributions | High — prevents long-run crisis immunity; requires Phase 5 panel premium flows | Sustained cycle dynamics |
| 7 — Lead-follow | Low in isolation — full value requires Phases 1–5 and relationship scores | Phenomena 3, 5, 7 |

Phase 5 is the critical path item. It is a prerequisite for Phase 6 (capital distributions need realistic premium volumes from variable-line policies), Phase 7 (follower behaviour requires a panel to subscribe into), and for deferred §7.4 voluntary exit mechanics. Phase 6 should begin immediately after Phase 5's panel assembly is validated, as it is a small addition to `handle_year_end` that does not change the quoting or settlement protocol. Phase 7 is the largest remaining phase and should not be started until Phases 5 and 6 are stable and their diagnostic signals are clean.
