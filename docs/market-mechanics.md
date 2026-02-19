# Market Mechanics

This is a living document. Mechanics are seeded from the reference literature and refined as implementation reveals what works. Sections marked *[TBD]* require calibration or design decisions not yet resolved.

---

## 1. Syndicate Actuarial Channel

The actuarial channel produces a long-run expected loss cost estimate for a submitted risk. It is one of two inputs to the syndicate's final quote; the other is the underwriter channel (§2).

### Inputs

- Risk characteristics: line of business, sum insured, territory, coverage trigger, attachment/limit structure.
- Syndicate's accumulated loss experience for that line, encoded as an EWMA of historical loss ratios (event-sourced where practical, mutable accumulator otherwise).
- An industry-benchmark loss ratio, published by the coordinator after each annual period (§8).

### Process

1. Apply line-of-business base loss cost from the syndicate's actuarial tables (parameterised per syndicate).
2. Blend own experience (EWMA loss ratio) with the industry benchmark using a credibility weight that increases with the syndicate's volume in that line. Low-volume syndicates weight the benchmark heavily; specialists weight their own experience heavily.
3. Apply risk-specific loadings: territory catastrophe factor, coverage trigger severity factor, attachment/limit adjustment (pro-rata by layer position).
4. Output: actuarial technical price (ATP) — the minimum premium at which the syndicate breaks even in expectation.

### Notes

- The ATP is not the quoted premium; it is a floor and an input.
- EWMA decay parameter controls how fast the syndicate forgets old experience. *[TBD: per-line or per-syndicate?]*

---

## 2. Syndicate Underwriter Channel

The underwriter channel reflects non-actuarial market intelligence: the current cycle position, relationship with the placing broker, and the observed lead quote (if any). It produces a market rate adjustment applied on top of the ATP.

### Inputs

- Current market cycle indicator (coordinator-published annually; derived from aggregate premium movement — see §8).
- Broker relationship score for the submitting broker.
- Lead quote (available only in follow-market mode; absent for the lead syndicate).
- Syndicate's risk appetite and cycle-sensitivity parameters.

### Process

1. **Cycle adjustment:** Syndicates with high cycle sensitivity shade their quotes toward the hard/soft market signal. In a hard market they price above ATP; in a soft market, competitive pressure pushes them toward ATP or below (floored by solvency constraints).
2. **Relationship adjustment:** A strong broker relationship reduces the loading the syndicate applies for placement friction. *[Not a discount — represents the syndicate's confidence in risk quality and broker due diligence.]*
3. **Lead-follow adjustment (follow mode only):** Follower syndicates observe the lead quote and allow it to pull their own price. A higher follow-weight parameter produces stronger herding toward the lead; a lower follow-weight produces more independent pricing.
4. Output: final quoted premium = ATP * (1 + underwriter adjustment).

### Capital constraint override

Before emitting the quote, the syndicate checks whether accepting the risk at that premium would breach its exposure limits, concentration limits, or solvency floor. If so, the syndicate either declines or quotes a premium high enough to make acceptance capital-neutral. The coordinator does not intervene in this decision.

---

## 3. Lead-Follow Quoting Process

Lloyd's operates a subscription market: a lead syndicate sets terms, and followers subscribe on those terms (or decline). The quoting round is orchestrated by the coordinator.

### Sequence

1. **Broker submission:** Broker selects a target panel and submits risk to the coordinator. Panel selection is driven by relationship scores and line specialism (see §4).
2. **Lead selection:** The coordinator identifies the lead syndicate — the panel member with the highest relationship score for that line. *[Alternative: broker nominates the lead explicitly. TBD.]*
3. **Lead quote:** Lead syndicate receives the risk in lead mode (no prior quote visible). It runs both channels (§1, §2) and emits a `QuoteIssued` event with its premium and capacity.
4. **Follow round:** Remaining panel members receive the risk and the lead quote. Each runs both channels in follow mode. They emit `QuoteIssued` (accept at stated premium or modified premium) or `QuoteDeclined`.
5. **Panel assembly:** Broker collects quotes. If sufficient capacity is assembled to cover the risk, it is placed. Broker emits `RiskPlaced` event listing participating syndicates, shares, and premiums.
6. **Shortfall handling:** If the panel is undersubscribed, the broker may approach additional syndicates (relationship-score ranked), or the risk is returned unplaced. *[TBD: retry limit, slip-down mechanics.]*

### Timing

All events in a quoting round share the same simulation timestamp. The ordering within the round (lead before followers) is enforced by event dependency, not wall-clock time.

---

## 4. Broker Relationship Score Evolution

Each broker maintains a relationship score per (syndicate, line-of-business) pair. Scores are initialised low for new relationships and evolve through placement activity.

### Update rules

**On successful placement (`RiskPlaced`):**
- All participating syndicates receive a positive score increment proportional to their share of the placement.
- The lead syndicate receives an additional increment for taking the lead position.

**On quote declined (`QuoteDeclined`):**
- Small negative adjustment. Repeated declines on submitted risks degrade the relationship.

**On syndicate non-performance (late payment, dispute):**
- Larger negative adjustment. *[Modelled as a coordinator-emitted event after loss settlement.]*

**Passive decay:**
- Scores decay toward a baseline at a slow exponential rate. Relationships that are not actively maintained fade over time.

### Score → routing behaviour

When assembling a panel, the broker ranks syndicates by their score for the relevant line and selects the top-N (where N is configurable per broker). A score threshold filters out syndicates below a minimum relationship quality. This produces the placement stickiness phenomenon (see `phenomena.md §5`).

### Initialisation

New syndicates enter with a score draw from a low-mean distribution for all brokers. They must win business (often at competitive prices) to build scores. *[TBD: whether an entering syndicate gets a one-time visibility boost from the coordinator to represent the real-world "capital introduction" process.]*

---

## 5. Syndicate Entry / Exit Triggers

This section describes the procedural rules governing when and how syndicates enter and leave the market. It is the mechanism behind phenomenon 6 (counter-cyclical capacity supply).

**Entry:** After each annual review, the coordinator checks whether the industry combined ratio and current market premium rate index cross an entry-attractiveness threshold. If so, it creates one or more new Syndicate agents with parameters drawn from a calibrated distribution (risk appetite, specialism, initial capital). New syndicates receive low initial relationship scores with all brokers. The entry process models the real Lloyd's capital introduction pathway.

**Exit (insolvency):** When a syndicate's capital falls below its solvency floor, the coordinator emits a `SyndicateInsolvent` event, removes the syndicate from active quoting, and transitions it to managed runoff (§6). The syndicate's bound policies continue to their expiry; new submissions are declined.

**Exit (voluntary runoff):** *[TBD — whether to model voluntary capital withdrawal during soft markets.]*

---

## 6. Managed Runoff and Central Fund

This section describes the institutional backstop that handles insolvent syndicates. It is a Lloyd's structural rule, not an emergent behaviour.

**Managed runoff:** On insolvency, the coordinator transitions the syndicate to a runoff state. The syndicate accepts no new submissions but continues settling claims on bound policies. It remains in the event stream until all outstanding policies have expired and all claims are settled, at which point it is retired.

**Central Fund:** Lloyd's operates a mutual Central Fund funded by annual levies on all active syndicates. When an insolvent syndicate in runoff cannot meet a claim from its own assets, the claim is paid from the Central Fund. The levy is a small annual deduction from each active syndicate's premium income; it is a friction cost in normal years and a larger drain in the aftermath of a catastrophe-driven insolvency wave.

**Design note:** The Central Fund is a welfare mechanism, not a cycle mechanism. It should not materially alter cycle period or amplitude. It does create a small pro-diversification incentive: syndicates with lower insolvency risk impose a lower expected levy burden on their peers, which over time could influence capital allocation norms.

---

## 7. Loss Event Mechanics

Losses fall into two structurally distinct classes. Each has different correlation properties across syndicates and different implications for capital and cycle dynamics.

### Attritional losses

- Arise from individual policy claims — fire, theft, liability, etc. — statistically independent across policies.
- A policy may generate zero or more claims over its term. Frequency and severity vary by line of business; both are calibration inputs.
- Claims are distributed to subscribing syndicates proportional to their line shares and reduce capital immediately.

### Catastrophe losses

- Arise from events that simultaneously affect all policies with exposure in the relevant region-peril (hurricane, earthquake, flood, etc.).
- Each affected policy's loss is proportional to its contribution to total regional exposure, capped at its policy limit.
- Losses are correlated across syndicates not by design but because brokers route similar risks to similar panels (§3, §4) — the cross-syndicate exposure overlap is an emergent property of the placement network.
- The simultaneous capital impact is the mechanism behind catastrophe-amplified capital crises (phenomena.md §2). *[TBD: severity distribution — heavy-tailed, but exact form and parameters are calibration work.]*

### Claims payment and knock-on effects

- Capital depletion below the solvency floor triggers §5 insolvency processing.
- Syndicate non-performance following a loss (late settlement, dispute) feeds back into broker relationship scores (§4).

### Actuarial feedback (closing the §1 loop)

- Each loss updates the syndicate's accumulated loss experience and revises its actuarial estimate — the primary input to §1.
- Syndicates learn from the full loss on a policy, not their proportional share. All syndicates on the same risk therefore converge toward the same long-run estimate regardless of line size. This is a structural rule, not a calibration choice.

---

## 8. Annual Coordinator Statistics

At the close of each annual period, the coordinator aggregates market-wide statistics and publishes them to all active syndicates. These are the market signal syndicates consume in their next pricing cycle and the primary outputs for benchmarking the simulation against Lloyd's empirical targets.

### Statistics published

- **Industry loss ratio:** total claims incurred divided by total premiums written. The primary signal of market profitability; feeds the cycle indicator consumed by the underwriter channel (§2).
- **Industry average premium rate:** market-wide average premium per unit of exposure, by line. Syndicates use this to position their own pricing relative to the market.
- **Aggregate claim frequency and severity:** the industry-benchmark component used in the §1 actuarial blend.
- **Active syndicate count and aggregate capacity:** signals how tight or loose market capacity is; an input to entry evaluation (§5).

### Design note

Statistics are a one-period-lagged signal — syndicates price for the coming year using the previous year's aggregate results. This lag is structural: combined with the multi-year EWMA in §1, it is one of the mechanisms that prevents immediate market equilibration and contributes to cycle persistence.

### Central Fund levy

*[TBD: whether to model explicitly.]* If Central Fund (§6) expenditure is tracked, an annual levy proportional to premium income is deducted from each active syndicate at this step.
