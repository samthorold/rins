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

*Add new phenomena here as the literature review and calibration work identify them.*
