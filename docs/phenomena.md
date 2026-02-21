# Target Phenomena

This is a living document. Phenomena are added as the literature review progresses and removed or merged when the simulation makes them redundant or subsumes them. Parameter values are not specified here — that is calibration work, not architecture.

---

## 1. Underwriting Cycle (Hard/Soft Market Alternation)

**What it is:** Aggregate market premium rates oscillate over multi-year cycles. Hard markets follow large loss events or capital shocks; soft markets emerge as capital is rebuilt and competition intensifies. Cycles in Lloyd's have historically run 5–10 years peak-to-peak.

**Why it matters:** The cycle is the most robust stylised fact in property-catastrophe reinsurance. A model that cannot reproduce it is not capturing the market's fundamental dynamics.

**Expected agent mechanism:** After a large loss event, syndicates reduce capacity (capital constraint), survivors raise rates. New capital enters attracted by elevated returns, capacity expands, competition drives rates down. The lag between loss, capital adjustment, and pricing response produces the oscillation. No agent targets a cycle — it emerges from individual capital management and competitive pricing responses.

---

## 2. Catastrophe-Amplified Capital Crisis

**What it is:** A large catastrophe (or correlated sequence) forces simultaneous syndicate losses that exceed what normal capital buffers can absorb, producing a wave of insolvencies or forced capital calls that temporarily removes a significant fraction of market capacity.

**Why it matters:** Distinguishes the model from a smooth mean-reversion story. Real markets exhibit fat-tailed loss distributions and non-linear responses; this phenomenon tests whether those properties propagate correctly through the agent layer.

**Expected agent mechanism:** Correlated exposure across syndicates (from similar risk selection) means a single large event triggers many simultaneous capital breaches. Insolvency processing by the coordinator removes affected syndicates, creating a capacity gap that cannot be filled instantly, producing the spike in residual rates. The key driver is the cross-syndicate correlation of held risk — a product of broker routing and syndicate risk appetite, not a hardcoded correlation parameter.

**Correlation mechanism note:** within a single catastrophe event, damage fractions across policies are sampled independently. The correlation across syndicates arises entirely from the *shared occurrence*: every syndicate writing US-SE property risks is struck by the same windstorm year. Per-policy severity remains independent; diversification *within* a single territory therefore does not reduce cat exposure materially. Only diversification *across* perils and territories reduces a syndicate's probability of being hit hard in a given year. This distinction is important: a syndicate writing 500 US-SE property risks is not more protected than one writing 50 — only one writing across US-SE, EU, and JP is.

---

## 3. Broker-Syndicate Network Herding

**What it is:** When syndicates on a risk panel observe a credible lead quote, followers converge on similar rates even if their own actuarial estimates differ. This produces clustered pricing and amplifies both under- and over-pricing errors across the market.

**Why it matters:** Herding is a channel through which mispricing propagates; it is also a mechanism for information transmission. Understanding which dominates depends on the relationship-strength network topology.

**Expected agent mechanism:** Broker relationship scores concentrate placement with a small number of high-relationship syndicates. Those syndicates are therefore disproportionately likely to be leads. Follower syndicates weight the lead quote heavily in their underwriter channel. When the lead misprices (either direction), the network amplifies the error. The herding strength is a function of the relationship-score concentration, which is itself an emergent property of past placement decisions.

---

## 4. Specialist vs. Generalist Divergence

**What it is:** Syndicates with narrow line-of-business specialisms outperform generalists during periods of stable loss experience in their specialty but are more vulnerable to correlated catastrophe shocks within that line.

**Why it matters:** Tests whether syndicate heterogeneity in risk appetite and specialism parameters produces realistic performance dispersion, rather than all syndicates converging on identical portfolios.

**Expected agent mechanism:** Specialism parameters bias syndicate risk selection toward specific lines. Brokers, through relationship routing, channel matching risks to specialists (better service, faster quotes, more competitive pricing on familiar risks). Specialists accumulate concentrated books that price well on average but have high tail correlation.

---

## 5. Relationship-Driven Placement Stickiness

**What it is:** Despite available capacity from new or lower-priced syndicates, brokers continue routing risks to established partners. Market share adjusts slowly even when pricing differences are material.

**Why it matters:** Placement stickiness damps the speed of competitive adjustment, lengthening cycle periods and creating periods of apparent market inefficiency. It is a behavioural friction with measurable empirical counterparts.

**Expected agent mechanism:** Broker relationship scores decay slowly and are reinforced by successful placements. New syndicates start with low scores and must win business at disadvantaged terms to build reputation. Existing relationships are retained even when the established syndicate is not the cheapest quote, because the broker internalises service quality, panel reliability, and future reciprocity.

---

## 6. Counter-cyclical Capacity Supply

**What it is:** After capital shocks and hard-market rate spikes, new syndicates enter the market attracted by elevated returns, gradually restoring capacity. During sustained soft markets or following catastrophe-driven insolvencies, syndicates exit — voluntarily or through insolvency. The aggregate effect is a lagged counter-cyclical adjustment of total market capacity that partially moderates rate spikes and prevents permanent post-catastrophe oligopolisation.

**Why it matters:** Without this dynamic, catastrophe-driven insolvencies produce permanently concentrated markets; with it, capital supply responds to profit signals over multi-year lags, producing the capacity rebuilding arc that characterises real hard-market recoveries. The delay between the profit signal and meaningful new capacity — because entrants take years to build broker relationships — is what allows hard markets to sustain elevated rates long enough to attract capital.

**Expected agent mechanism:** When industry aggregate statistics cross an entry-attractiveness threshold the coordinator creates a new Syndicate with low initial broker relationship scores. The new syndicate must compete for placements to build scores; brokers route to it slowly because it ranks below established partners. Its capacity contribution is therefore delayed. The interaction of this lag with phenomena 1 (underwriting cycle) and 7 (concentration surge) produces the full multi-year recovery arc.

---

## 7. Post-Catastrophe Market Concentration Surge

**What it is:** A catastrophe-amplified capital crisis removes multiple syndicates simultaneously, concentrating market share among surviving firms — disproportionately those that are larger, more diversified, or better-capitalised. Surviving syndicates temporarily dominate panel assembly, enabling above-normal pricing and deepening their broker relationships until new entrants erode their position over subsequent years.

**Why it matters:** Concentration dynamics are a distinct, measurable consequence of catastrophe events beyond the initial capital shock. A model that produces insolvencies but not the resulting oligopolisation — and its resolution via new entrant competition — is missing the full recovery arc. The resolution of the surge requires phenomenon 6 (counter-cyclical capacity supply) to operate correctly; the two phenomena validate each other.

**Expected agent mechanism:** Insolvency events remove syndicates from the coordinator's active set, mechanically raising surviving syndicates' market share. With fewer competitors, surviving syndicates receive more placement attempts, receive stronger relationship-score reinforcement, and can price above ATP. Elevated returns trigger entry (phenomenon 6), and entrants gradually restore competition as their scores accumulate. No agent targets concentration — the surge and its unwinding are entirely the product of capital management, insolvency processing, relationship-score dynamics, and panel assembly.

---

## 8. Geographic and Peril Accumulation Risk

**What it is:** Catastrophe losses are geographically and peril-correlated: a single event strikes all syndicates holding exposure in the affected region simultaneously. The routing patterns that emerge from relationship scores and specialism parameters produce systematic accumulation of correlated exposure within syndicates and across panels. Syndicates that fail to spread exposure across regions and perils face amplified catastrophe losses relative to the market average, increasing their insolvency probability.

**Why it matters:** Accumulation is the mechanism through which effective diversification breaks down under fat-tailed catastrophe risk. It explains why catastrophe-amplified crises (phenomenon 2) are more severe than attritional loss experience predicts — a portfolio's effective diversification depends on its geographic spread, not just its size. If the model reproduces this correctly, syndicates face a natural selection pressure toward diversification that arises from capital management constraints rather than from a hardcoded diversification goal.

**Expected agent mechanism:** Catastrophe events affect all active risks in a struck region simultaneously. A syndicate with concentrated regional exposure — driven by specialism parameters or broker routing to a specialist — receives a disproportionate share of the catastrophe loss. Syndicate capital management (concentration limits on peril/region exposure) is the internal defence. Syndicates that enforce strict limits survive large catastrophes at the cost of foregone volume; those that relax limits to maximise premium accumulate correlated exposure and face amplified insolvency risk. The selection pressure emerges from individual capital decisions, not from any market-level diversification rule.

**Accumulation at the Insured level:** accumulation risk exists on the demand side too. An Insured holding multiple risks in the same territory — a manufacturing group with plants across US-SE, for example — accumulates correlated ground-up losses across all of its assets in a single cat event. The sum of GUL across policies can far exceed any single policy limit, and the insured absorbs whatever portion falls below attachments or above limits. This creates demand-side pressure: insureds that suffer repeated large events may restructure their coverage (higher limits, lower attachments, multi-year contracts) or seek alternative risk transfer. That feedback is not yet modelled but is a future target, because it would alter the size and structure of the submission population over time.

---

## 9. Experience Rating and Insured Risk Quality

*Not yet implemented. This section documents the intended phenomenon so the feature can be designed against it.*

**What it is:** In a real market, underwriters observe an insured's loss history and adjust terms accordingly — surcharging after bad years, restricting limits after large events, declining renewal for chronically unprofitable accounts. Over time, insureds with poor loss records face higher attachments, smaller panel participation, or outright declination. The insured pool self-selects: high-quality risks stay cheaply insured; chronic loss generators are pushed toward specialist markets or out of the insured pool entirely.

**Why it matters:** Experience rating is one of the primary mechanisms through which the market achieves risk segmentation. Without it, all insureds face similar terms regardless of their actual loss history, and the market cannot distinguish adverse selection from legitimate risk. With it, the submission population evolves endogenously as the market reprices and restructures access based on accumulated experience. This affects cycle dynamics because a shrinking insured pool reduces premium volume even as rates rise, damping the apparent profitability of a hard market.

**Foundation in the current model:** the Insured agent already accumulates `total_ground_up_loss_by_year` from `InsuredLoss` events. This per-insured GUL history is the natural input to an experience rating mechanism. The data is available; what is not yet implemented is the feedback path from that history into syndicate pricing and panel-assembly decisions.

**Expected agent mechanism:** After each annual period, syndicates with sufficient loss history for an insured apply a credibility-weighted surcharge to the ATP for that insured's next renewal. Insureds with GUL consistently below attachment attract competition and tighter terms; those with GUL repeatedly exceeding the policy limit face higher attachments or panel defection. The coordinator does not enforce this — it emerges from individual syndicate decisions to accept or decline renewals based on accumulated experience. The emergent phenomenon: chronically high-GUL insureds face shrinking panel participation, eventually concentrating in specialist syndicates or exiting the market; low-GUL insureds attract broad panel competition and declining rates.

*Add new phenomena here as the literature review and calibration work identify them.*
