# Target Phenomena

This is a living document. Phenomena are added as the literature review progresses and removed or merged when the simulation makes them redundant or subsumes them. Parameter values are not specified here — that is calibration work, not architecture.

**Status badges:** `[CONFIRMED]` = reliably reproduced, matches theoretical predictions; `[EMERGING]` = visible and measurable in current output; `[PARTIAL]` = some aspects emergent, key drivers not yet implemented; `[PLANNED]` = designed, requires planned mechanics before it can emerge; `[TBD]` = identified but not yet specified.

| # | Phenomenon | Status |
|---|---|---|
| 0 | Risk Pooling (Law of Large Numbers) | CONFIRMED |
| 1 | Underwriting Cycle | PARTIAL |
| 2 | Catastrophe-Amplified Capital Crisis | PARTIAL |
| 3 | Broker-Syndicate Network Herding | PLANNED |
| 4 | Specialist vs. Generalist Divergence | PLANNED |
| 5 | Relationship-Driven Placement Stickiness | PARTIAL |
| 6 | Counter-cyclical Capacity Supply | PLANNED |
| 7 | Post-Catastrophe Market Concentration Surge | PLANNED |
| 8 | Geographic and Peril Accumulation Risk | PARTIAL |
| 9 | Experience Rating and Insured Risk Quality | PLANNED |
| 10 | Layer-Position Premium Gradient | PLANNED |
| 11 | Reinsurance Contagion Cascade | TBD |
| 12 | Pricing Rule Evolution Under Selection Pressure | PLANNED |
| 13 | Reserve Development / Adverse Loss Reserve | PLANNED |
| 14 | Cat Model Homogeneity as Systemic Risk | TBD |

---

## 0. Risk Pooling (Law of Large Numbers) `[CONFIRMED]`

**What it is:** Risk pooling is a benefit to *insureds*, not insurers. Each individual faces highly uncertain annual losses: most years modest attritional claims, rare years a large hit. Insurance lets the insured swap that uncertain outcome for a fixed, known premium. The insurer can offer this exchange because the Law of Large Numbers makes *aggregate* losses predictable — the CV of total attritional claims declines as 1/√N as the pool grows.

Attritional and catastrophe losses behave structurally differently under pooling. Attritional losses are independent across insureds — each draws from the same distribution independently — so pooling compresses aggregate variance efficiently (CV scales as 1/√N). Catastrophe losses arise from a single shared occurrence that strikes all exposed risks simultaneously; adding more risks in the same territory does not reduce the CV of the cat component, because the dominant variance is the event-level severity which is perfectly correlated across the pool. Only diversification across uncorrelated perils and territories reduces cat variance.

**Why it matters:** Validates that the loss architecture produces correct individual-to-aggregate variance compression for attritional losses while correctly failing to compress cat losses. The distinction is not cosmetic: it is why catastrophe-exposed books remain volatile regardless of their size, and why cat loading in premium is structurally different from attritional loading.

**Already measurable:** Canonical run (seed=42, 100 insureds, 3 territories, 20 analysis years + 5yr warmup):
- Attritional individual insured GUL: CV **1.19**. Market-average attritional GUL per insured: CV **0.11**. CV ratio **10.6×** — matches the LLN prediction of √100 = 10× almost exactly. Pooling operates correctly for independent losses.
- Cat individual GUL (14 cat-year observations): CV **1.32**. Market-average cat GUL per insured: CV **0.53**. CV ratio **2.5×** — partial-portfolio shocks (~33% of insureds per event) reduce but do not eliminate cat volatility. Within a single territory the CV ratio would be ~1.0× (full correlation for exposed insureds); the territory routing lifts it to 2.5× at the market level, confirming that geographic diversification compresses cat variance as theory predicts — but incompletely relative to attritional pooling (10.6×).
- Both predictions confirmed without hardcoding: the attritional compression arises from independent per-policy LogNormal draws; the partial cat compression arises from territory routing that limits each event to ~33% of the portfolio rather than the full correlated book.

No hardcoded smoothing produces this; it arises from the LogNormal attritional model (independent per-policy draws) and the shared-occurrence cat model.

---

## 1. Underwriting Cycle (Hard/Soft Market Alternation) `[PARTIAL]`

**What it is:** Aggregate market premium rates oscillate over multi-year cycles. Hard markets follow large loss events or capital shocks; soft markets emerge as capital is rebuilt and competition intensifies. Cycles in Lloyd's have historically run 5–10 years peak-to-peak.

**Why it matters:** The cycle is the most robust stylised fact in property-catastrophe reinsurance. A model that cannot reproduce it is not capturing the market's fundamental dynamics.

**Full cycle mechanism (Cummins & Outreville 1987; Venezian 1985):**

1. Soft market — capital abundant, competition drives rates toward ATP, CRs approach or exceed 100%.
2. Shock — catastrophe or reserve deterioration depletes capital.
3. Hard market — capacity scarce (capital-linked limits tighten), rates rise, CRs fall below 100%, profits rebuild capital.
4. Capital entry — elevated returns attract new capital (new syndicates, ILS, sidecars), capacity expands, competition drives rates back down.
5. Repeat.

Steps 1 and 2 are now mechanically represented: the `market_ap_tp_factor` (AP/TP ratio) compresses rates toward the soft floor (0.90 × TP) during benign multi-year stretches, and hardens them (up to 1.40 × TP) following loss spirals. Step 4 (capital entry) is implemented as a coordinator action that spawns new insurers when `market_ap_tp_factor > 1.10` with a 1-year cooldown; it is expected to damp the permanent hard market and introduce cyclical capacity expansion, but the full oscillatory cycle has not yet been confirmed in output.

**Currently visible (canonical seed=42, 200 analysis years, 5yr warmup):**
- Rate-on-line oscillates between ~12–20% RoL. Both directions of rate movement are present; hard-market spikes revert within 1–2 years (empirical Lloyd's cycles: 5–10 years peak-to-peak). 17 years exceed 100% LossR over 200 years, all cat-driven. Capital entry fires correctly: 15 entrants over 200 years, clustering after severe events. Market grows from 8 insurers to 23. Capital per insurer stabilises near initial levels once Phase 6 distributions are active; no insolvencies despite a 212% LossR year.

**Hard market collapse — remaining structural gaps:** individual pricing (Phase 1), variable line sizes (Phase 5), demand elasticity (Phase 4), and capital distributions (Phase 6) are all active. The hard-market duration gap (1–2 years observed vs. 5–10 years empirical) has two structural causes:

1. **No rising supply curve for capital entry.** A sustained hard market spawns new entrants each year with no declining marginal attractiveness signal. The nth entrant requires a higher expected return than the (n-1)th in practice — the easiest capital deploys first, and marginal capital carries higher formation cost and risk premium. Without this, post-catastrophe entry is too fast and too flat, collapsing capacity scarcity within 1–2 years. See market-mechanics.md §7.1.

2. **No investment income.** Syndicates earn returns on the Premium Trust Fund and Funds at Lloyd's. In high-rate environments (2022–2024: 4–5%), investment income subsidises underwriting losses and delays the market's recognition of crisis severity; in low-rate environments (2010–2021), syndicates depend entirely on underwriting profit, making hard markets sharper and longer. Venezian (1985) and Cummins & Outreville (1987) identify the interest rate channel as a co-driver of cycle period — the empirically observed 5–10 year cycle aligns with the interest rate cycle, not purely with cat event frequency. Without investment income, the simulation's cycle is driven entirely by capital depletion, producing shorter hard markets than the real world exhibits. See market-mechanics.md §4.6.

**Remaining mechanisms for full cycle oscillation:**
- Rising supply curve for capital entry (market-mechanics.md §7.1) — flat entry signal currently allows too-rapid capacity restoration.
- Investment income on reserves (market-mechanics.md §4.6) — absent; the interest rate channel is a co-driver of cycle period (Venezian 1985; Cummins & Outreville 1987).
- Reserve development lag (phenomenon §13) — adverse development creates a secondary capital shock 12–24 months post-event, sustaining hard markets; absent from current settlement model.
- Reinstatement premiums (market-mechanics.md §2.1) — post-cat within-year premium income absent; dampens net loss impact and amplifies within-year rate signal.
- Demand elasticity — buyers must be able to adjust coverage quantity (limit, deductible, self-insurance) so demand responds to price and amplifies both the hard and soft phases.
- Competitive individual pricing — per-insurer blending (Phase 1) is active; but the coordinator formula still supplies the common market signal. Until the market signal is itself derived from observable competitor quotes (§4.5 Phase D), the coordinator injects a coordinating function that cannot distinguish itself from emergent herding.

**Hardcoded vs. emergent cycle elements:**

The mechanisms above partially *produce* the cycle and partially *encode* it. The distinction matters for validation: a truly emergent phenomenon can be compared against empirical calibration targets as an independent test; a parameterized one is tautologically consistent with its parameters.

*Truly emergent (arise from first principles):*
- Capital depletion via cat claims → exposure limit tightening → reduced capacity → `Dropped#` rising (the capacity crunch follows mechanically from capital-linked limits, not from a hardcoded scarcity signal).
- New insurer entry triggered by `market_ap_tp_factor > 1.10` → gradual score-building → delayed capacity restoration (the timing lag emerges from relationship dynamics, not from a hardcoded delay parameter).
- Per-insurer EWMA attritional ELF updating → individual ATP movements in response to own experience.
- Capital depletion adjustment in `own_ap_tp_factor` → depleted insurers individually quote above the market signal without coordinator instruction.

*Currently parameterized (not emergent — encoded learned equilibria):*
- `capacity_uplift = 0.05 if dropped_count > 10` — the coordinator injects a fixed price increase when supply is tight. This is the capacity-scarcity pricing response pre-specified rather than derived; it pre-empts the individual reasoning that should produce it (see market-mechanics.md §4.5 Phase A).
- `clamp(factor, 0.90, 1.40)` — cycle amplitude is designer-bounded. The simulation cannot discover its own natural amplitude; these bounds enforce an assumption about it.
- `MARKET_FLOOR_WEIGHT = 0.30` — herding intensity is fixed. The coordination outcome is hardcoded rather than emerging from information structure and selection pressure (see market-mechanics.md §4.5 Phase D).
- 3-year CR lookback — effective pricing memory is a designer choice, not a learned result.
- `credibility = min(own_years / 5.0, 1.0)` — quality-blind credibility ramp; a mature insurer with five benign years is as credible as one with five volatile years (see market-mechanics.md §4.5 Phase C).

The path from parameterized to emergent is described in market-mechanics.md §4.5. Until those phases are complete, cycle dynamics should be interpreted with the caveat that amplitude, memory, and herding intensity are calibration inputs, not outputs.

*AP/TP mechanism active (step 1). Per-insurer blending active (Phase 1). Capital entry active (step 4). Cycle signal present in both directions. Full oscillatory cycle with emergent amplitude requires §4.5 Phases A–D.*

---

## 2. Catastrophe-Amplified Capital Crisis `[PARTIAL]`

**What it is:** A large catastrophe (or correlated sequence) forces simultaneous syndicate losses that exceed what normal capital buffers can absorb, producing a wave of insolvencies or forced capital calls that temporarily removes a significant fraction of market capacity.

**Why it matters:** Distinguishes the model from a smooth mean-reversion story. Real markets exhibit fat-tailed loss distributions and non-linear responses; this phenomenon tests whether those properties propagate correctly through the agent layer.

**Expected agent mechanism:** Correlated exposure across syndicates (from similar risk selection) means a single large event triggers many simultaneous capital breaches. Insolvency processing by the coordinator removes affected syndicates, creating a capacity gap that cannot be filled instantly, producing the spike in residual rates. The key driver is the cross-syndicate correlation of held risk — a product of broker routing and syndicate risk appetite, not a hardcoded correlation parameter.

**Correlation mechanism note:** within a single catastrophe event, damage fractions across policies are sampled independently. The correlation across syndicates arises entirely from the *shared occurrence*: every syndicate writing US-SE property risks is struck by the same windstorm year. Per-policy severity remains independent; diversification *within* a single territory therefore does not reduce cat exposure materially. Only diversification *across* perils and territories reduces a syndicate's probability of being hit hard in a given year. This distinction is important: a syndicate writing 500 US-SE property risks is not more protected than one writing 50 — only one writing across US-SE, EU, and JP is.

*Capital losses land correctly and insolvency processing is active. The shared-occurrence criterion is met: all homogeneous insurers are struck simultaneously in a cat year, confirming cross-syndicate correlation arises from the shared occurrence rather than a hardcoded parameter. In the 200-year run, year 124 produced 6 cat events and a 225% LossR — the most severe event observed — yet dropped TotalCap by only 9.5% (5.37B → 4.86B) and produced no insolvencies.*

*The structural barrier to full crisis emergence is capital accumulation, not insufficient cat severity. With TotalCap at 8B and the theoretical maximum single-year GUL at ~1.25B, insolvency is structurally impossible regardless of cat clustering: the worst conceivable year is a 16% capital draw at year-200 capital levels. Capital distributions (roadmap Phase 6) are required to maintain a capital level at which severe cat years can genuinely threaten solvency over long-run simulations. Until then, this phenomenon can confirm the crisis mechanics are wired correctly (shared occurrence, simultaneous multi-insurer losses, correct capital accounting) but cannot produce the insolvency cascade that defines it.*

---

## 3. Broker-Syndicate Network Herding `[PLANNED]`

**What it is:** When syndicates on a risk panel observe a credible lead quote, followers converge on similar rates even if their own actuarial estimates differ. This produces clustered pricing and amplifies both under- and over-pricing errors across the market.

**Why it matters:** Herding is a channel through which mispricing propagates; it is also a mechanism for information transmission. Understanding which dominates depends on the relationship-strength network topology.

**Expected agent mechanism:** Broker relationship scores concentrate placement with a small number of high-relationship syndicates. Those syndicates are therefore disproportionately likely to be leads. Follower syndicates weight the lead quote heavily in their underwriter channel. When the lead misprices (either direction), the network amplifies the error. The herding strength is a function of the relationship-score concentration, which is itself an emergent property of past placement decisions.

*Requires: relationship scores and lead-follow mode (market-mechanics.md §5).*

---

## 4. Specialist vs. Generalist Divergence `[PLANNED]`

**What it is:** Syndicates with narrow line-of-business specialisms outperform generalists during periods of stable loss experience in their specialty but are more vulnerable to correlated catastrophe shocks within that line.

**Why it matters:** Tests whether syndicate heterogeneity in risk appetite and specialism parameters produces realistic performance dispersion, rather than all syndicates converging on identical portfolios.

**Expected agent mechanism:** Specialism parameters bias syndicate risk selection toward specific lines. Brokers, through relationship routing, channel matching risks to specialists (better service, faster quotes, more competitive pricing on familiar risks). Specialists accumulate concentrated books that price well on average but have high tail correlation.

*Requires: specialism parameters and relationship-score routing (market-mechanics.md §5).*

---

## 5. Relationship-Driven Placement Stickiness `[PARTIAL]`

**What it is:** Despite available capacity from new or lower-priced syndicates, brokers continue routing risks to established partners. Market share adjusts slowly even when pricing differences are material.

**Why it matters:** Placement stickiness damps the speed of competitive adjustment, lengthening cycle periods and creating periods of apparent market inefficiency. It is a behavioural friction with measurable empirical counterparts.

**Implemented mechanism (Phase 3):** `Broker.relationship_scores` accumulate +1.0 per `PolicyBound` and decay ×0.80 per `YearEnd`. `on_coverage_requested` solicits the top-k (canonical: 4 of N) insurers by score — incumbents get first look; new entrants start at 0.0 and must price below incumbents to win business and build score. Gini coefficient of bound-policy count is tracked per year (0.03–0.19 in canonical run). The routing mechanism is active; the stickiness dynamic is emerging.

**Remaining gap:** With a single-insurer panel and cheapest-wins selection, relationship score affects *who is solicited* but not *who wins* — the cheapest insurer in the solicited set always wins. Full stickiness (incumbent wins even if not cheapest) requires either a multi-insurer panel where the lead's price sets the anchor, or a buyer preference factor that bends the acceptance rule toward incumbent insurers. Both are Phase 5 prerequisites.

*Partially satisfies: routing concentration. Requires Phase 5 lead-follow for full stickiness effect.*

---

## 6. Counter-cyclical Capacity Supply `[PLANNED]`

**What it is:** After capital shocks and hard-market rate spikes, new syndicates enter the market attracted by elevated returns, gradually restoring capacity. During sustained soft markets or following catastrophe-driven insolvencies, syndicates exit — voluntarily or through insolvency. The aggregate effect is a lagged counter-cyclical adjustment of total market capacity that partially moderates rate spikes and prevents permanent post-catastrophe oligopolisation.

**Why it matters:** Without this dynamic, catastrophe-driven insolvencies produce permanently concentrated markets; with it, capital supply responds to profit signals over multi-year lags, producing the capacity rebuilding arc that characterises real hard-market recoveries. The delay between the profit signal and meaningful new capacity — because entrants take years to build broker relationships — is what allows hard markets to sustain elevated rates long enough to attract capital.

**Expected agent mechanism:** When industry aggregate statistics cross an entry-attractiveness threshold the coordinator creates a new Syndicate with low initial broker relationship scores. The new syndicate must compete for placements to build scores; brokers route to it slowly because it ranks below established partners. Its capacity contribution is therefore delayed. The interaction of this lag with phenomena 1 (underwriting cycle) and 7 (concentration surge) produces the full multi-year recovery arc.

**Simulation evidence for capacity supply (200-year run).** Capital entry fires correctly: 7 entrants over 200 years cluster after severe years (yrs 45, 92, 124–126, 134, 138). Market grows from 8 insurers to 15 by year 138. The entry lag is visible — `Dropped#` spikes in the year of a severe event, then falls over the following 2–3 years as entrants build relationship scores. The `Gini` column confirms new entrants start with near-zero share and build gradually. The entry half of this phenomenon is operational.

The *exit* half is not yet operational. With `line_size = 1.0` fixed, insurers cannot reduce supply without exiting entirely (binary). Binary exit was removed (Phase 2). Variable line sizes (roadmap Phase 5) are the prerequisite for the exit half: insurers reduce their offered line fraction as rates soften, producing continuous soft-market supply contraction. Until Phase 5 is active, the counter-cyclical mechanism is one-sided (entry works, exit is missing), which explains the monotonic capacity growth (8 → 15 insurers) with no soft-market correction.

**Calibration target:** new capacity should absorb the majority of the capacity shortfall within 2–3 years of the shock, consistent with the Bermuda classes (12–18 month formation, 2–3 year relationship-building before material panel share). The timing lag is the mechanism that prevents immediate rate collapse after a hard market begins and produces the elevated-rate period during which capital recovers.

*Requires: variable line sizes (Phase 5) for the exit half; entry is already active (market-mechanics.md §7.1).*

---

## 7. Post-Catastrophe Market Concentration Surge `[PLANNED]`

**What it is:** A catastrophe-amplified capital crisis removes multiple syndicates simultaneously, concentrating market share among surviving firms — disproportionately those that are larger, more diversified, or better-capitalised. Surviving syndicates temporarily dominate panel assembly, enabling above-normal pricing and deepening their broker relationships until new entrants erode their position over subsequent years.

**Why it matters:** Concentration dynamics are a distinct, measurable consequence of catastrophe events beyond the initial capital shock. A model that produces insolvencies but not the resulting oligopolisation — and its resolution via new entrant competition — is missing the full recovery arc. The resolution of the surge requires phenomenon 6 (counter-cyclical capacity supply) to operate correctly; the two phenomena validate each other.

**Expected agent mechanism:** Insolvency events remove syndicates from the coordinator's active set, mechanically raising surviving syndicates' market share. With fewer competitors, surviving syndicates receive more placement attempts, receive stronger relationship-score reinforcement, and can price above ATP. Elevated returns trigger entry (phenomenon 6), and entrants gradually restore competition as their scores accumulate. No agent targets concentration — the surge and its unwinding are entirely the product of capital management, insolvency processing, relationship-score dynamics, and panel assembly.

*Requires: insolvency processing (market-mechanics.md §6) and syndicate entry/exit (market-mechanics.md §7).*

---

## 8. Geographic and Peril Accumulation Risk `[PARTIAL]`

**What it is:** Catastrophe losses are geographically and peril-correlated: a single event strikes all syndicates holding exposure in the affected region simultaneously. The routing patterns that emerge from relationship scores and specialism parameters produce systematic accumulation of correlated exposure within syndicates and across panels. Syndicates that fail to spread exposure across regions and perils face amplified catastrophe losses relative to the market average, increasing their insolvency probability.

**Why it matters:** Accumulation is the mechanism through which effective diversification breaks down under fat-tailed catastrophe risk. It explains why catastrophe-amplified crises (phenomenon 2) are more severe than attritional loss experience predicts — a portfolio's effective diversification depends on its geographic spread, not just its size. If the model reproduces this correctly, syndicates face a natural selection pressure toward diversification that arises from capital management constraints rather than from a hardcoded diversification goal.

**Expected agent mechanism:** Catastrophe events affect all active risks in a struck region simultaneously. A syndicate with concentrated regional exposure — driven by specialism parameters or broker routing to a specialist — receives a disproportionate share of the catastrophe loss. Syndicate capital management (concentration limits on peril/region exposure) is the internal defence. Syndicates that enforce strict limits survive large catastrophes at the cost of foregone volume; those that relax limits to maximise premium accumulate correlated exposure and face amplified insolvency risk. The selection pressure emerges from individual capital decisions, not from any market-level diversification rule.

**Accumulation at the Insured level:** accumulation risk exists on the demand side too. An Insured holding multiple risks in the same territory — a manufacturing group with plants across US-SE, for example — accumulates correlated ground-up losses across all of its assets in a single cat event. The sum of GUL across policies can far exceed any single policy limit, and the insured absorbs whatever portion falls below attachments or above limits. This creates demand-side pressure: insureds that suffer repeated large events may restructure their coverage (higher limits, lower attachments, multi-year contracts) or seek alternative risk transfer. That feedback is not yet modelled but is a future target, because it would alter the size and structure of the submission population over time.

**Insurer-side accumulation management is now active:** `Insurer` tracks live `cat_aggregate` (WindstormAtlantic sum_insured across in-force policies) and enforces `max_cat_aggregate` at quote time — emitting `LeadQuoteDeclined` when the limit would be breached. This is the capital-management defence that creates selection pressure described in the agent mechanism above. Canonical limit: 750M USD per insurer (15 × ASSET_VALUE, ~15 policies before saturation).

Geographic routing is now active: each cat event targets one territory, and ~33% of the insured population is exposed per event. Specialist vs. generalist *divergence* (syndicates concentrating exposure in one territory vs. spreading across all three) requires broker specialism routing (market-mechanics.md §5) before it can emerge as a measurable phenomenon; the structural prerequisite (multi-territory distribution) is now satisfied.

---

## 9. Experience Rating and Insured Risk Quality `[PLANNED]`

*Not yet implemented. This section documents the intended phenomenon so the feature can be designed against it.*

**What it is:** In a real market, underwriters observe an insured's loss history and adjust terms accordingly — surcharging after bad years, restricting limits after large events, declining renewal for chronically unprofitable accounts. Over time, insureds with poor loss records face higher attachments, smaller panel participation, or outright declination. The insured pool self-selects: high-quality risks stay cheaply insured; chronic loss generators are pushed toward specialist markets or out of the insured pool entirely.

**Why it matters:** Experience rating is one of the primary mechanisms through which the market achieves risk segmentation. Without it, all insureds face similar terms regardless of their actual loss history, and the market cannot distinguish adverse selection from legitimate risk. With it, the submission population evolves endogenously as the market reprices and restructures access based on accumulated experience. This affects cycle dynamics because a shrinking insured pool reduces premium volume even as rates rise, damping the apparent profitability of a hard market.

**Foundation in the current model:** the Insured agent already accumulates `total_ground_up_loss_by_year` from `InsuredLoss` events. This per-insured GUL history is the natural input to an experience rating mechanism. The data is available; what is not yet implemented is the feedback path from that history into syndicate pricing and panel-assembly decisions.

**Expected agent mechanism:** After each annual period, syndicates with sufficient loss history for an insured apply a credibility-weighted surcharge to the ATP for that insured's next renewal. Insureds with GUL consistently below attachment attract competition and tighter terms; those with GUL repeatedly exceeding the policy limit face higher attachments or panel defection. The coordinator does not enforce this — it emerges from individual syndicate decisions to accept or decline renewals based on accumulated experience. The emergent phenomenon: chronically high-GUL insureds face shrinking panel participation, eventually concentrating in specialist syndicates or exiting the market; low-GUL insureds attract broad panel competition and declining rates.

*GUL history exists in the current model; feedback path not yet implemented.*

---

## 10. Layer-Position Premium Gradient `[PLANNED]`

**What it is:** In a layered insurance programme, the premium per unit of limit (rate on line, ROL) decreases systematically with attachment height. Working/primary layers — hit by every moderate loss and all catastrophes — attract ROLs of 15–30%; upper/remote layers — triggered only by extreme events — attract ROLs of 1–8%. This gradient is not negotiated ad hoc: it emerges from the underlying expected loss distribution of each layer and from the competition among syndicates with different risk appetites for each position.

**Why it matters:** The gradient creates a segmented pricing dynamic. After a catastrophe event, the working layer's loss experience deteriorates directly and immediately (every cat loss penetrates the bottom of the tower); upper layers may not be triggered at all in a moderate cat year. This means the hardening post-catastrophe is not uniform: primary-layer rates spike more than upper-layer rates, producing differential pricing pressure by layer position. The converse also holds: sustained soft markets compress upper-layer ROLs more than lower-layer ROLs because remote-layer capacity is abundant (ILS, new entrants) and the expected loss is small enough that competition drives the price toward the capital cost floor.

**Expected agent mechanism:** Syndicates specialising in lower-layer positions accumulate working losses every year (attritional and moderate cat penetration). Their loss experience is noisier but more predictable; pricing is data-rich. After a bad loss year, their EWMA-based ATP rises sharply and they increase quotes, hardening the primary-market segment. Syndicates specialising in upper layers experience long quiet periods punctuated by large single-event losses. Their ATP rises only when a large cat event actually triggers their layer; their cycle timing is therefore event-driven rather than continuous. This asynchrony between primary-layer and remote-layer hardening is a second-order cycle effect: the overall hard market rate index averages two segments that are hardening for different reasons at different speeds.

**Connection to other phenomena:** Layer-position dynamics are a refinement of phenomenon 4 (Specialist vs. Generalist Divergence) — layer-position specialism adds a vertical dimension to the existing line-of-business horizontal dimension. They also modulate phenomenon 1 (Underwriting Cycle): the full-market ROL index masks the divergent dynamics in each layer segment, which can produce apparent cycle dampening in aggregate that conceals structural stress in the working-layer segment.

*Requires: programme structures (market-mechanics.md §9.2).*

---

## 11. Reinsurance Contagion Cascade `[TBD]`

**What it is:** When a major reinsurer becomes insolvent following a catastrophic loss, multiple primary insurers that purchased reinsurance from it simultaneously lose their expected cession recovery. This creates a correlated secondary shock across the primary market — distinct from, and potentially worse than, the direct primary losses — as each affected primary insurer faces a larger-than-expected net retained loss. The cascade can trigger primary insolvencies even among firms whose gross exposure was well within tolerance.

**Why it matters:** The primary market and the reinsurance market are coupled through counterparty exposure. A model that treats reinsurance as a parametric net-retention adjustment captures the normal-times capacity benefit of reinsurance but misses this tail-event feedback loop. Paulson & Staber (2021) identify it as the mechanism through which reinsurance is *simultaneously* stabilising (dampens individual-firm first-order cat losses) and destabilising (amplifies systemic failure when the reinsurer itself fails). The interaction between the two effects is non-monotone and empirically observed in events such as the near-failure of several Lloyd's Names syndicates following Katrina/Rita/Wilma (2005), where reinsurance recoveries from partially insolvent vehicles did not materialise at full face value.

**Expected agent mechanism:** reinsurers are modelled as agents with their own capital. When a catastrophe event occurs, reinsurer agents receive aggregate cession recoveries from multiple primaries simultaneously. If total cessions exceed the reinsurer's capital, it becomes insolvent and settles at cents on the dollar. Each primary insurer that expected a full recovery instead receives a fractional settlement, booking the difference as an unexpected additional net loss. If that loss depletes a primary's capital, the primary becomes insolvent in turn. The cascade propagates through the bipartite primary-reinsurer network.

**Conditions for emergence:** the cascade requires two structural elements: (a) reinsurers are not infinitely capitalised — they have finite balance sheets subject to the same cat event risk as primaries; (b) multiple primaries share a common reinsurer — concentration of cessions with a single reinsurer creates network fragility. Both are realistic: reinsurance markets are concentrated (Swiss Re, Munich Re, Hannover Re together hold a large fraction of industry capacity), and large catastrophes are the same events that deplete both primary and reinsurance capital simultaneously.

*Requires: reinsurer agents (market-mechanics.md §10). Not yet designed; tracked here as a target phenomenon.*

---

## 12. Pricing Rule Evolution Under Selection Pressure `[PLANNED]`

**What it is:** Over many simulation years (200+), syndicates with miscalibrated pricing sensitivities — too soft in hard markets, too rigid in soft markets, too credulous of own experience, too slavish to the market signal — underperform, lose business, or go insolvent. Surviving syndicates accumulate capital and broker relationships. The distribution of pricing parameters across the active population converges toward a natural equilibrium that is the empirically observed market behaviour.

**Why it matters:** The current underwriter channel (market-mechanics.md §4.2) hardcodes the equilibrium outcome as initial conditions: `capacity_uplift = 0.05`, `MARKET_FLOOR_WEIGHT = 0.30`, a 5-year credibility ramp, clamp bounds of [0.90, 1.40]. These produce realistic-looking output but are epistemically closed — the simulation cannot challenge or falsify its own pricing assumptions. A model in which these parameters are free and selection pressure converges them treats the market equilibrium as a *finding*, not an assumption. If the converged values match the current calibration, the calibration is validated. If they diverge, the calibration is corrected.

**Expected mechanism:** Syndicates enter with pricing sensitivity parameters drawn from distributions (`cr_sensitivity`, `capacity_sensitivity`, `market_weight`). A syndicate that discounts the market signal too heavily prices idiosyncratically; if its own model is wrong it accumulates losses. One that follows the market signal too mechanically cannot respond faster than the market's 3-year CR lag. Selection pressure — operating through capital depletion, insolvency, and relationship-score attrition — eliminates the worst-calibrated syndicates. The distribution of surviving parameters narrows over time, and the central tendency of that distribution is the endogenous answer to "what should the 30% floor be?" and "how long should the pricing memory be?"

**Connection to §1 (Underwriting Cycle):** The cycle amplitude, period, and asymmetry that emerge from this selection process are the calibration targets for §1. Until §12 is running, the §1 cycle dynamics depend on the hardcoded parameters, which is the tension documented in §1 above. §12 resolves that tension by making the parameters endogenous.

**Requirements:**
- Per-insurer heterogeneous pricing sensitivity parameters (`cr_sensitivity`, `capacity_sensitivity`, `market_weight`).
- Parameter variation at syndicate entry (draw from distribution, or mutate from a parent syndicate's parameters — representing the institutional learning that new capital brings).
- Extended simulation horizon: 200+ years for meaningful distributional convergence.
- Bühlmann-Straub credibility replacing linear ramp (§4.5 Phase C).
- Observable market signal replacing coordinator broadcast (§4.5 Phase D).

*Requires: §4.5 Phases A–D (market-mechanics.md). Long-run target; not in current development cycle.*

---

## 13. Reserve Development / Adverse Loss Reserve `[PLANNED]`

**What it is:** Lloyd's syndicates operate a 3-year account. When an underwriting year closes (after three years), any remaining open liabilities are reinsured-to-close (RITC). Between policy inception and closure, reserves develop: favourable development (releases) or adverse development (strengthening). In soft-market years, initial reserves are often conservative and are subsequently released, artificially boosting reported profits. After major events (Katrina 2005, Ian 2022), reserves are progressively strengthened as total loss estimates grow — creating a secondary capital shock 12–24 months after the event, distinct from the initial ClaimSettled deduction.

**Why it matters:** Reserve development is a lagged capital shock with a distinctive signature that amplifies and extends hard markets. It does not appear in the year of loss; it surfaces in the one to two years after. This is one reason why post-Katrina hardness persisted through 2007–2008: reinsurers kept strengthening Atlantic wind reserves while new events (Ike, Gustav) were still materialising. The current simulation books all claims immediately at `ClaimSettled`, with no reserving lag. This understates the duration of post-catastrophe capital impairment and is a structural reason why simulated hard markets are too short.

Adverse development also interacts with phenomenon §1 (Underwriting Cycle) through the soft-market reserve-release mechanism. During benign years, releasing excess reserves appears as a profit, inflating the syndicate's combined ratio and masking deteriorating underwriting quality. When the releases exhaust, the true combined ratio steps up abruptly — a sudden hardening trigger with no new catastrophe event. The empirical record shows this pattern repeatedly (Lloyd's 1988–1992 crisis was amplified by exhausted attritional reserves; US P&C soft market of the late 1990s ended partly via reserve deficiency recognition in 2001).

**Expected agent mechanism:** At `PolicyBound`, the syndicate sets an initial loss reserve (best estimate of ultimate claims). Reserve is updated annually from new information (loss development factors applied to open claims). At `YearEnd`, if the revised reserve exceeds the current held amount, the insurer books a reserve strengthening that debits capital; if less, a release credits capital. Over a 3-year development tail, the aggregate reserve movement can equal or exceed the initial year's claims — making reserve development a meaningful capital volatility source independent of new cat events.

*Requires: reserve development model in `Insurer` (initial reserve at bind, annual IBNR development, close-out at year 3). No new events needed — `CapitalDistributed`-style reserve-adjustment event would suffice.*

---

## 14. Cat Model Homogeneity as Systemic Risk `[TBD]`

**What it is:** When all syndicates rely on the same vendor catastrophe model (RMS, AIR, Verisk), they simultaneously misprice the same events in the same direction. After Andrew (1992), all models underestimated Gulf Coast residential construction vulnerability. After Harvey (2017), all models missed inland freshwater flood losses outside FEMA 100-year floodplains. After Ida (2021), all models missed the remnant tropical system's flood track into the Northeast. In each case, a correlated pricing error existed across the market *before* the event, because the model population was structurally homogeneous.

**Why it matters:** This is a distinct systemic risk mechanism from behavioural herding (phenomenon §3). Behavioural herding amplifies a *known* price signal through social imitation — syndicates intentionally follow the lead quote. Model homogeneity amplifies an *unknown* pricing error through shared structural assumptions — syndicates independently arrive at the same wrong answer. The two mechanisms interact: behavioural herding concentrates exposure in syndicates that happen to share the same model, compounding the systemic consequence of a model error. Paulson & Staber (2021, *JEIC*) — cited in market-mechanics.md §10 — find that "under risk-model homogeneity, reinsurance partially alleviates systemic fragility — but only because it diversifies exposure across balance sheets that remain correlated in pricing assumptions." The implication is that diversification of reinsurance counterparties does not address the fundamental problem if all counterparties use the same EAL estimate.

**Expected agent mechanism:** Syndicates enter with a `cat_model_params` draw from a distribution rather than a single shared value. The distribution of `pml_damage_fraction_200` across the active population represents market model diversity. When the true 1-in-200 event turns out to differ from the market consensus (a bad draw from the physical cat distribution), syndicates whose model parameters were closest to the truth survive better; those furthest from the truth face correlated outsized losses. The degree of model homogeneity — the variance of `cat_model_params` across active syndicates — is then a measurable market property that the simulation can track and that selection pressure (phenomenon §12) should, over time, converge toward a less homogeneous equilibrium as miscalibrated syndicates exit. The current "1-in-3 aggressive new entrant" mechanism (`pml_damage_fraction_override`) is a binary approximation of this continuous distribution.

**Connection to §12 (Pricing Rule Evolution):** Model parameter diversity is one of the free parameters that §12's selection mechanism should converge. The question "what is the equilibrium distribution of cat model parameters across a competitive market?" is an endogenous finding, not a calibration input. Until §12 is running, model heterogeneity is a designer choice.

*Not yet designed. Requires: design of `cat_model_params` distribution at entry; mechanism for realised-vs-modelled divergence to produce differential capital impacts; long-run tracking of model parameter distribution. Tracked here as a target phenomenon given the Paulson/Staber finding.*

---

*Add new phenomena here as the literature review and calibration work identify them.*
