# Simulation Roadmap

This document captures the phased plan for evolving the simulation's supply and demand mechanics toward full underwriting cycle emergence. Each phase is motivated by a specific structural gap identified in the current model, states a falsifiable hypothesis, and specifies the diagnostic signals that would confirm or deny it.

Phases are ordered by expected impact per implementation cost and by logical dependency. Each phase should be completed with a failing test written first, verified against the invariant suite, and evaluated against the diagnostic criteria before the next phase begins.

---

## Diagnosis: current structural gaps

The canonical run (seed=42, 8×150M insurers, 100 insureds, 25 analysis years) shows rate-on-line oscillating directionally but collapsing prematurely: a 1.82pp rate spike after the year-16 double-cat collapses fully within 3 years despite year 18 also producing an 85% loss ratio. Capital accumulates monotonically from 1.20B to 1.80B over 25 years. No insolvencies. These are signs of three structural gaps:

**Gap 1 — Coordinator-broadcast pricing.** All insurers apply the same `market_ap_tp_factor`, computed from the market-aggregate 3-year CR. A capital-depleted incumbent prices identically to a flush new entrant. There is no mechanism for insurers to hold rates while competitors soften, so the hard market collapses as soon as the aggregate CR signal normalises — regardless of individual capital recovery status.

**Gap 2 — No voluntary exit, and uncapped entry count.** `[Fixed — Phase 2]` Insurers enter when AP/TP > 1.10 but never leave except via insolvency. Supply is a ratchet: it expands in hard markets and holds in soft ones. The resulting monotonic capital accumulation prevents the soft-market phase of the cycle from developing.

A companion sub-gap on the entry side: the 1-year cooldown constrains the *timing* of entry but not the *total* number of entrants. A sustained 10-year hard market would spawn 10 new insurers on top of the starting 8 — more than doubling capacity — with no declining marginal attractiveness signal. The real market has a finite pool of willing capital with a rising supply curve: the nth entrant requires higher expected returns than the (n-1)th because it draws from progressively less-experienced or higher-hurdle capital sources. The model assumes flat, infinite supply. Both the exit floor and the entry ceiling are needed for a symmetric supply response; fixing only voluntary exit leaves the entry side uncapped.

**Gap 3 — Inelastic demand.** All 100 insureds buy full coverage at fixed sum_insured. Demand does not respond to price. In the real market, hard-market rate spikes cause buyers to raise retentions, reduce limits, or self-insure, shrinking effective demand and absorbing some of the supply contraction. Soft-market rates attract buyers back in. Without this, new entrant capital can only clear by reducing rates — it has no volume to absorb.

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
- 5+ year hard-market duration — *not yet reached*; extended from 3 to ~4 years; Phase 2 (voluntary exit) needed for full confirmation.

**Does not fix.** Demand inelasticity (Gap 3) and supply ratchet (Gap 2). The rate erosion mechanism shifts from administrative to competitive, but there is still no demand-side resistance and no voluntary exit to close the soft-market floor.

---

## Phase 2 — Voluntary exit (soft-market capital withdrawal) `[DONE — 2026-02-27]`

**Mechanism.** After each `YearEnd`, each insurer evaluates whether to continue writing new business. If the insurer's own 3-year average combined ratio exceeds `runoff_cr_threshold` (canonical: 1.05) *and* capital > `capital_exit_floor × initial_capital` (canonical: 0.90), it enters runoff: stops accepting new submissions, lets in-force policies expire naturally, and logs an `InsurerExited` event. The broker removes it from the round-robin. It re-enters when the market AP/TP signal rises above 1.10.

**Primary hypothesis — confirmed.** A clear two-sided supply cycle emerges. Year 8 (CombR 112.7%, 1 cat) triggers 3 voluntary exits, dropping the market from 5 to 2 active insurers. The 2 survivors write profitably (CombR ~77–99%) but capacity is severely constrained: 43–51 policies dropped per year throughout years 9–20 vs 0 in years 6–8. Year 20 (3 cat events, LR 150.4%, AP/TP 1.43) triggers a re-entry wave of 9 new insurers in a single year, fully restoring capacity by year 21 (Dropped# drops from 51 to 0). The market then oscillates through a new exit wave (years 22–24, –4, –2, –1) as the soft-market CR signal triggers voluntary withdrawal again. Supply is no longer a one-sided ratchet.

**Secondary hypotheses.**
- TotalCap(B) non-monotonic — *confirmed*: capital falls from 1.29B (yr 19) to 1.22B (yr 20) during the cat cluster, recovers to 1.65B post-entry wave, then drifts back to 1.58B as exits re-fire. The monotonic accumulation of Phase 1 is broken.
- Dropped# rises in capacity-constrained years — *confirmed*: 43–51 dropped/year in the 2-insurer period (yrs 9–20), dropping to 0 immediately on re-entry. Demand-driven rejection not yet distinguishable (Phase 3).
- Hard market amplitude increases — *confirmed*: AP/TP reaches 1.43 in year 20 vs a maximum of ~1.08 in the Phase 1 canonical run. Fewer surviving insurers hold rates higher before new entrants appear.

**Does not fix.** Demand inelasticity (Gap 3). Buyers still purchase fixed-SI coverage at fixed terms regardless of rate level. Without demand response, the cycle is supply-side only — it oscillates, but amplitude and period are driven entirely by capital flows rather than the two-sided equilibrium the real market exhibits.

**Does not fix.** Demand inelasticity. Buyers still buy fixed-SI at fixed terms regardless of rate level. Without demand response, the cycle is supply-side only — it oscillates, but the amplitude and period are driven entirely by capital flows rather than by the two-sided equilibrium the real market exhibits.

---

## Phase 3 — Demand elasticity (heterogeneous reservation prices)

**Mechanism.** Replace the single uniform `max_rate_on_line = 0.15` with a distribution across insureds. The simplest parametric form: `max_rol ~ Uniform(low, high)` or `LogNormal(mu, sigma)`, calibrated so that at the canonical 6–8% rate band nearly all insureds participate, but above 10–12% a measurable fraction opt out (raising retentions, self-insuring, or placing with lower-quality markets not modelled).

The `Dropped#` column then measures a mix of supply-constrained and demand-constrained non-placements. When rates spike, some insureds voluntarily withdraw; when rates soften, they return. The in-force policy count becomes a function of both capacity and price.

A richer extension: buyers with above-average GUL history have lower reservation prices (they've seen what losses cost and value coverage more), while low-loss-history buyers are more price-sensitive. This connects to phenomenon 9 (Experience Rating) and makes the insured pool quality endogenous.

**Primary hypothesis.** In hard-market years (Rate% > 9%), in-force policy count falls as marginal buyers price out. In soft-market years (Rate% < 6.5%), in-force count rises toward the full 100. The effective demand curve is downward-sloping rather than vertical, absorbing some of the capacity movement and moderating rate swings. Cycle period lengthens slightly; amplitude is lower than Phase 2 alone.

**Secondary hypotheses.**
- `QuoteRejected` (demand-driven) is distinguishable from `SubmissionDropped` (supply-driven) in hard markets; the mix shifts toward demand rejection as rates spike.
- The insured pool in hard markets is adversely selected toward high-loss-history buyers (low-risk buyers price out first), mildly elevating the loss ratio above ATP expectations.

**Diagnostics.** Track `QuoteRejected` vs `SubmissionDropped` separately. Plot in-force policy count vs Rate% across years — a downward slope confirms demand elasticity is active.

**Does not fix.** Competitive quoting. The broker still routes to one insurer at a time; there is no mechanism for buyers to shop or for insurers to compete for the same submission simultaneously.

---

## Phase 4 — Relationship-ranked routing (competitive quoting infrastructure already built)

**What is already implemented.** The concurrent solicitation and cheapest-wins selection mechanism described in earlier versions of this phase is already in production. `Broker::on_coverage_requested` solicits `k = quotes_per_submission` insurers in parallel (canonical: `None` → all N insurers); `on_lead_quote_issued` tracks responses in `PendingQuote` and retains the cheapest; `QuotePresented` carries the winning `(insurer_id, premium)`. Phase 1's per-insurer `own_ap_tp_factor` already produces meaningful price dispersion (CV 0.07–0.18 post-warmup), so competition over quotes is real.

**Remaining work.** Round-robin start-index routing means the subset of insurers solicited when `k < N` is positionally determined, not relationship-determined. Replacing the round-robin start index with relationship-score ranking is the only architectural change needed: the broker selects the top-k insurers by score for the relevant line, solicits them concurrently, and the cheapest-wins logic runs unchanged.

This is the architectural prerequisite for the lead-follow subscription model (Phase 5), which requires a nominated lead (highest-score insurer) distinct from the follower pool.

**Primary hypothesis.** With relationship-ranked routing, submission flow concentrates on high-score incumbents. New entrants — starting with low scores — win business disproportionately by pricing below incumbents, building scores gradually. Market share becomes a function of relationship score × price competitiveness, not routing position. Persistently expensive incumbents lose share to lower-priced new entrants and (via Phase 2) eventually exit.

**Secondary hypotheses.**
- Gini coefficient of market share across insurers rises post-cat (concentration among surviving incumbents) and falls in soft markets (new entrant competition).
- Phenomenon 4 (Specialist vs Generalist Divergence) can begin to emerge: insurers with aggressive pricing on specific risk types win disproportionate share of those risks.

**Diagnostics.** Per-insurer bound-policy count, by year. Market share Gini coefficient measurable from `PolicyBound.insurer_id` counts.

---

## Phase 5 — Lead-follow subscription market

**Mechanism.** Full Lloyd's subscription model: broker nominates a lead insurer based on relationship score. Lead quotes in lead mode (no prior quote visible, full individual pricing from Phase 1). Followers observe the lead quote and shade ±Δ based on their own actuarial view and relationship. If total subscribed capacity reaches the required limit, the risk is placed as a panel policy split across multiple insurers.

This is the prerequisite for phenomena 3 (Broker-Syndicate Network Herding), 5 (Relationship-Driven Placement Stickiness), and 7 (Post-Catastrophe Market Concentration Surge).

**Primary hypothesis.** Lead syndicates with strong relationship scores set the market price for a risk; followers amplify pricing errors in both directions. Market-wide rate movements are faster in one direction (herding amplifies hardening post-cat) and stickier in the other (relationship stickiness slows softening as established leads hold rates). Cycle asymmetry — faster hardening than softening — matches the empirical record.

---

## Sequencing rationale

Phases are ordered by two criteria: (a) independent value — does the phase produce a testable hypothesis in isolation, or does it only matter in combination with later phases? (b) architectural dependency — does a later phase require the earlier one's infrastructure?

| Phase | Independent value | Unlocks |
|---|---|---|
| 1 — Individual pricing | High — rate dispersion and hard-market duration immediately testable | Phase 4, Phase 5 |
| 2 — Voluntary exit | High — supply-side oscillation testable in isolation | Full cycle confirmation |
| 3 — Demand elasticity | Medium — cycle modulation measurable, but cycle must already exist | Phenomenon 9 |
| 4 — Competitive quoting | Medium — market share dynamics; needs Phase 1 to produce price dispersion worth competing over | Phase 5 |
| 5 — Lead-follow | Low in isolation — full value requires Phases 1–4 and relationship scores | Phenomena 3, 5, 7 |

Phases 1 and 2 are independent of each other and could be developed in parallel. Phase 3 requires a functioning cycle (Phases 1+2) to be meaningful. Phases 4 and 5 require Phase 1's individual pricing to produce competitive dynamics worth modelling.
