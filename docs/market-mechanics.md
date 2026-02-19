# Market Mechanics

This is a living document. Mechanics are seeded from the reference literature and refined as implementation reveals what works. Sections marked *[TBD]* require calibration or design decisions not yet resolved.

---

## 1. Syndicate Actuarial Channel

The actuarial channel produces a long-run expected loss cost estimate for a submitted risk. It is one of two inputs to the syndicate's final quote; the other is the underwriter channel (§2).

### Inputs

- Risk characteristics: line of business, sum insured, territory, coverage trigger, attachment/limit structure.
- Syndicate's accumulated loss experience for that line, encoded as an EWMA of historical loss ratios (event-sourced where practical, mutable accumulator otherwise).
- An industry-benchmark loss ratio, updated by the coordinator after each major loss event.

### Process

1. Apply line-of-business base loss cost from the syndicate's actuarial tables (parameterised per syndicate).
2. Blend own experience (EWMA loss ratio) with market benchmark using a credibility weight that increases with the syndicate's volume in that line. Low-volume syndicates weight the benchmark heavily; specialists weight their own experience heavily.
3. Apply risk-specific loadings: territory catastrophe factor, coverage trigger severity factor, attachment/limit adjustment (pro-rata by layer position).
4. Output: actuarial technical price (ATP) — the minimum premium at which the syndicate breaks even in expectation.

### Notes

- The ATP is not the quoted premium; it is a floor and an input.
- EWMA decay parameter controls how fast the syndicate forgets old experience. *[TBD: per-line or per-syndicate?]*
- Credibility blending formula: `blend = own_weight * ewma_ratio + (1 - own_weight) * market_ratio`, where `own_weight = volume / (volume + credibility_threshold)`.

---

## 2. Syndicate Underwriter Channel

The underwriter channel reflects non-actuarial market intelligence: the current cycle position, relationship with the placing broker, and the observed lead quote (if any). It produces a market rate adjustment applied on top of the ATP.

### Inputs

- Current market cycle indicator (coordinator-published; updated periodically from aggregate premium movement).
- Broker relationship score for the submitting broker.
- Lead quote (available only in follow-market mode; absent for the lead syndicate).
- Syndicate's risk appetite and cycle-sensitivity parameters.

### Process

1. **Cycle adjustment:** Syndicates with high cycle sensitivity shade their quotes toward the hard/soft market signal. In a hard market they price above ATP; in a soft market, competitive pressure pushes them toward ATP or below (floored by solvency constraints).
2. **Relationship adjustment:** A strong broker relationship reduces the loading the syndicate applies for placement friction. *[Not a discount — represents the syndicate's confidence in risk quality and broker due diligence.]*
3. **Lead-follow adjustment (follow mode only):** Follower syndicates observe the lead quote and apply a weighted blend: `follow_quote = alpha * lead_quote + (1 - alpha) * own_atp_adjusted`, where `alpha` is the follow-weight parameter. High alpha = strong herding; low alpha = more independent pricing.
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
5. **Panel assembly:** Broker collects quotes. If sufficient capacity is assembled (sum of follower capacities + lead capacity ≥ risk sum insured * target fill fraction), the risk is placed. Broker emits `RiskPlaced` event listing participating syndicates, shares, and premiums.
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

*Sections to be added as implementation progresses: loss event distribution mechanics, insolvency processing sequence, syndicate entry/exit triggers, aggregate statistics published by the coordinator.*
