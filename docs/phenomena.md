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
| 5 | Relationship-Driven Placement Stickiness | PLANNED |
| 6 | Counter-cyclical Capacity Supply | PLANNED |
| 7 | Post-Catastrophe Market Concentration Surge | PLANNED |
| 8 | Geographic and Peril Accumulation Risk | PARTIAL |
| 9 | Experience Rating and Insured Risk Quality | PLANNED |
| 10 | Layer-Position Premium Gradient | PLANNED |
| 11 | Reinsurance Contagion Cascade | TBD |

---

## 0. Risk Pooling (Law of Large Numbers) `[CONFIRMED]`

**What it is:** Risk pooling is a benefit to *insureds*, not insurers. Each individual faces highly uncertain annual losses: most years modest attritional claims, rare years a large hit. Insurance lets the insured swap that uncertain outcome for a fixed, known premium. The insurer can offer this exchange because the Law of Large Numbers makes *aggregate* losses predictable — the CV of total attritional claims declines as 1/√N as the pool grows.

Attritional and catastrophe losses behave structurally differently under pooling. Attritional losses are independent across insureds — each draws from the same distribution independently — so pooling compresses aggregate variance efficiently (CV scales as 1/√N). Catastrophe losses arise from a single shared occurrence that strikes all exposed risks simultaneously; adding more risks in the same territory does not reduce the CV of the cat component, because the dominant variance is the event-level severity which is perfectly correlated across the pool. Only diversification across uncorrelated perils and territories reduces cat variance.

**Why it matters:** Validates that the loss architecture produces correct individual-to-aggregate variance compression for attritional losses while correctly failing to compress cat losses. The distinction is not cosmetic: it is why catastrophe-exposed books remain volatile regardless of their size, and why cat loading in premium is structurally different from attritional loading.

**Already measurable:** Canonical run (seed=42, 100 insureds, 55 active policies in steady state, 20 analysis years):
- Attritional individual insured GUL: CV **1.07**. Market-average attritional GUL per insured: CV **0.12**. CV ratio **9.1×** — closely tracks the LLN prediction of √55 = 7×. Pooling operates correctly for independent losses.
- Cat individual GUL (10 cat-year observations): CV **0.52**. Market-average cat GUL per insured: CV **0.52**. CV ratio **1.00×** — pooling within a single territory provides zero diversification for correlated losses.
- Both predictions confirmed without hardcoding: the attritional compression arises from the independent per-policy LogNormal draws; the cat non-compression arises from the shared-occurrence model where all active policies are struck by the same LossEvent.

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

**Currently visible (canonical seed=42, years 3–22):**
- Rate-on-line rises monotonically: 6.23% (year 3) → 8.26% (year 13) → 8.06% (year 22). No mean-reversion.
- Coverage contracts from 5.00B → ~2.85–3.00B after year 7 (double-cat, Cats#=2) and stays there.
- `Dropped#` rises from 0 (years 3–4) to 40–44 (years 13–22): ~40% of submissions find no available capacity.
- Total capital depleted from 3.62B → ~2.75B by year 8; recovers slowly to ~3.09B by year 22 but never returns to the pre-stress level within the simulation horizon.
- `ApTp` column in year table tracks the market factor; in benign years it should soften toward 0.90, in cat years spike toward 1.30–1.40.

The supply-side contraction and price hardening are correct in sign and qualitatively realistic. With AP/TP active, quiet periods now generate soft-market rate compression (AP/TP → 0.90), creating the vulnerability gap that makes the next catastrophe more damaging — and triggering the hard-market recovery arc. The cycle signal is now present in both directions.

**Remaining mechanism — capital entry dynamics (step 4):** entry is now AP/TP-driven (`market_ap_tp_factor > 1.10`, 1-year cooldown) — the profit-signal mechanism that empirically drives Bermuda class formation. For full cycle oscillation the new entrants must build sufficient broker relationships to materially expand capacity (relationship-building lag not yet implemented), and soft-market voluntary exit (§7.4) must be added to close the lower tail of the cycle.

*AP/TP mechanism active (step 1). Capacity entry active — AP/TP-driven (step 4). Full oscillatory cycle requires soft-market exit mechanism and broker relationship-building dynamics.*

---

## 2. Catastrophe-Amplified Capital Crisis `[PARTIAL]`

**What it is:** A large catastrophe (or correlated sequence) forces simultaneous syndicate losses that exceed what normal capital buffers can absorb, producing a wave of insolvencies or forced capital calls that temporarily removes a significant fraction of market capacity.

**Why it matters:** Distinguishes the model from a smooth mean-reversion story. Real markets exhibit fat-tailed loss distributions and non-linear responses; this phenomenon tests whether those properties propagate correctly through the agent layer.

**Expected agent mechanism:** Correlated exposure across syndicates (from similar risk selection) means a single large event triggers many simultaneous capital breaches. Insolvency processing by the coordinator removes affected syndicates, creating a capacity gap that cannot be filled instantly, producing the spike in residual rates. The key driver is the cross-syndicate correlation of held risk — a product of broker routing and syndicate risk appetite, not a hardcoded correlation parameter.

**Correlation mechanism note:** within a single catastrophe event, damage fractions across policies are sampled independently. The correlation across syndicates arises entirely from the *shared occurrence*: every syndicate writing US-SE property risks is struck by the same windstorm year. Per-policy severity remains independent; diversification *within* a single territory therefore does not reduce cat exposure materially. Only diversification *across* perils and territories reduces a syndicate's probability of being hit hard in a given year. This distinction is important: a syndicate writing 500 US-SE property risks is not more protected than one writing 50 — only one writing across US-SE, EU, and JP is.

*Capital losses land correctly and insolvency processing is active: `InsurerInsolvent` is emitted on first zero-crossing and insolvent insurers decline future quotes. Year 7 (double-cat, Cats#=2) drove a $480M+ market-wide capital drop from 3.52B → 2.75B (−22%) — the shared-occurrence criterion is met. No insolvencies have occurred in canonical runs because 500M USD/insurer absorbs even a double-cat year. The capital-crisis cascade requires either more severe draws, tighter capital ratios, or an accumulation of multiple bad years without intervening capital entry.*

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

## 5. Relationship-Driven Placement Stickiness `[PLANNED]`

**What it is:** Despite available capacity from new or lower-priced syndicates, brokers continue routing risks to established partners. Market share adjusts slowly even when pricing differences are material.

**Why it matters:** Placement stickiness damps the speed of competitive adjustment, lengthening cycle periods and creating periods of apparent market inefficiency. It is a behavioural friction with measurable empirical counterparts.

**Expected agent mechanism:** Broker relationship scores decay slowly and are reinforced by successful placements. New syndicates start with low scores and must win business at disadvantaged terms to build reputation. Existing relationships are retained even when the established syndicate is not the cheapest quote, because the broker internalises service quality, panel reliability, and future reciprocity.

*Requires: relationship scores (market-mechanics.md §5).*

---

## 6. Counter-cyclical Capacity Supply `[PLANNED]`

**What it is:** After capital shocks and hard-market rate spikes, new syndicates enter the market attracted by elevated returns, gradually restoring capacity. During sustained soft markets or following catastrophe-driven insolvencies, syndicates exit — voluntarily or through insolvency. The aggregate effect is a lagged counter-cyclical adjustment of total market capacity that partially moderates rate spikes and prevents permanent post-catastrophe oligopolisation.

**Why it matters:** Without this dynamic, catastrophe-driven insolvencies produce permanently concentrated markets; with it, capital supply responds to profit signals over multi-year lags, producing the capacity rebuilding arc that characterises real hard-market recoveries. The delay between the profit signal and meaningful new capacity — because entrants take years to build broker relationships — is what allows hard markets to sustain elevated rates long enough to attract capital.

**Expected agent mechanism:** When industry aggregate statistics cross an entry-attractiveness threshold the coordinator creates a new Syndicate with low initial broker relationship scores. The new syndicate must compete for placements to build scores; brokers route to it slowly because it ranks below established partners. Its capacity contribution is therefore delayed. The interaction of this lag with phenomena 1 (underwriting cycle) and 7 (concentration surge) produces the full multi-year recovery arc.

**Simulation evidence for its absence:** without capital entry, the canonical run produces a permanent hard market (phenomenon 1 note): `Dropped#` rising to 40–44, rate-on-line climbing to 8%+, coverage staying at ~57–60% of total insured assets — stable for a decade rather than cycling. This is the diagnostic signature of missing counter-cyclical supply. Once capital entry fires, the expected signature would be: elevated `Dropped#` → new insurer added → `Dropped#` falls → rate softens → new cycle phase.

**Calibration target:** new capacity should absorb the majority of the capacity shortfall within 2–3 years of the shock, consistent with the Bermuda classes (12–18 month formation, 2–3 year relationship-building before material panel share). The timing lag is the mechanism that prevents immediate rate collapse after a hard market begins and produces the elevated-rate period during which capital recovers.

*Requires: syndicate entry/exit (market-mechanics.md §7).*

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

The phenomenon itself is not yet measurable as an *emergent divergence* because all insureds are in the same territory and all insurers have identical parameters. Cross-territory accumulation contrast and specialist vs. generalist divergence require multi-territory specialism (market-mechanics.md §5) before they can be measured.

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

*Add new phenomena here as the literature review and calibration work identify them.*
