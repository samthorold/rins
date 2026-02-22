# Market Mechanics

This is a living document. Mechanics are ordered by concept dependency — you cannot understand a policy without an asset, or a claim without an occurrence. Each section carries a status badge:

- `[ACTIVE]` — implemented and running in canonical config; code location cited.
- `[PARTIAL]` — structural scaffold exists but simplified relative to the full spec.
- `[PLANNED]` — designed, not yet started.
- `[TBD]` — requires design decisions before implementation.

---

## Summary table

| Mechanic | Status | Primary implementation |
|---|---|---|
| Asset / Peril / Occurrence model | ACTIVE | `src/perils.rs`, `src/insured.rs` |
| Attritional loss scheduling | ACTIVE | `src/simulation.rs::schedule_attritional_claims_for_policy` |
| Catastrophe loss distribution | ACTIVE | `src/market.rs::on_loss_event` |
| Policy terms (full-value, zero attachment) | ACTIVE (PARTIAL — full-value simplification of layer mechanics) | `src/market.rs::on_insured_loss` |
| Annual policy expiry | ACTIVE | `src/market.rs::expire_policies` |
| Fixed-rate pricing (underwriter channel) | ACTIVE (PARTIAL — simplification of underwriter channel) | `src/insurer.rs::underwriter_premium` |
| Lead-follow quoting (round-robin) | ACTIVE (PARTIAL — simplification of lead-follow) | `src/broker.rs` |
| Actuarial channel (structural scaffold) | PARTIAL — prior-based ATP, no EWMA yet | `src/insurer.rs::actuarial_price` |
| Underwriter channel / cycle adjustment | PLANNED | — |
| Broker relationship scores | PLANNED | — |
| Syndicate entry / exit | PLANNED | — |
| Annual coordinator statistics | PLANNED | — |
| Quarterly renewal seasonality | PLANNED | — |
| Programme structures / towers | PLANNED | — |
| Experience rating (per-insured surcharge) | PLANNED | — |
| Central Fund / managed runoff | TBD | — |

---

## 1. World Model `[ACTIVE]`

Insurance is risk transfer: an Insured holds assets with economic value; a peril occurrence converts some of that value into a loss; a policy transfers a defined tranche of that loss to the market.

### §1.1 Assets `[ACTIVE]`

An **Asset** is a unit of economic value owned by an Insured. It is characterised by:

- `sum_insured` — total replacement value, in currency units. This is the ceiling on any physical loss from a single occurrence.
- `territory` — geographic/peril zone. Determines which occurrences the asset is exposed to.
- `perils_covered` — the set of Peril classes that can generate loss against this asset.

An Insured may own multiple Assets (different territories, different perils). Multiple Perils may affect the same Asset simultaneously if they share territory.

**Current simplification:** each Insured owns exactly one risk with a single `sum_insured`, one territory (`AtlanticCoast`), and the full canonical peril set.

### §1.2 Perils `[ACTIVE]`

A **Peril** is a hazard category. Two classes exist:

**Attritional** — many small, statistically independent occurrences: slips, minor fires, everyday property damage. Uncorrelated across assets. Predictable in aggregate; the total annual loss for N homogeneous policies is a compound distribution (sum of N independent draws), not N times a single draw.

**Catastrophe** — rare, large occurrences (hurricane, earthquake, flood). A single physical event simultaneously affects all assets in its territory. Correlated by construction. The dominant source of year-to-year capital volatility.

Currently implemented perils: `WindstormAtlantic` (cat, Pareto damage model) and `Attritional` (LogNormal damage model). Both live in `src/perils.rs`.

### §1.3 Occurrences and ground-up loss `[ACTIVE]`

An **Occurrence** is a single physical event of a Peril. It has an intensity represented by a **damage fraction** ∈ [0, 1].

```
GUL = damage_fraction × sum_insured
```

`GUL ≤ sum_insured` is a hard invariant: an occurrence cannot destroy more value than exists. The GUL is a real-world fact, independent of any insurance contract.

**Catastrophe occurrence mechanics** (`src/market.rs::on_loss_event`): when a `LossEvent` fires, the coordinator draws **one** damage fraction from the peril's `DamageFractionModel` (Pareto, clipped to [0, 1]). This single draw is shared across every affected policy — it represents the event's physical intensity field, which is identical for all assets in the same territory. The coordinator fans out to all active policies in the matching (territory, peril) index; every affected policy receives `GUL = shared_fraction × sum_insured`.

**Why a shared fraction:** physical damage at a given location is determined by the event's intensity field. Two neighbouring assets exposed to the same windstorm experience the same wind speed. Modelling this as a single shared draw captures the dominant correlation correctly. Residual asset-level variation (construction quality, micro-siting) is second-order and not included in the base model.

**Attritional occurrence mechanics** (`src/simulation.rs::schedule_attritional_claims_for_policy`): at `PolicyBound`, a per-policy Poisson scheduler samples the expected number of attritional occurrences for the year and schedules each as a future `InsuredLoss` event (no `LossEvent` ancestor). Each occurrence draws an **independent** damage fraction; independence across policies is preserved.

---

## 2. Insurance Contracts `[PARTIAL]`

### §2.1 Policy terms and layer mechanics `[PARTIAL]`

A policy covers a defined tranche of ground-up loss — the layer [attachment, attachment + limit]:

```
gross = min(GUL, limit)
net   = gross − attachment   →  insured loss (0 if GUL ≤ attachment)
```

Three loss layers per occurrence:

| Layer | What it represents | Quantity |
|---|---|---|
| Asset value | Total economic value exposed | `sum_insured` |
| Ground-up loss (GUL) | Physical damage, independent of insurance | `damage_fraction × sum_insured` |
| Insured loss | Market's share after policy terms | `min(GUL, limit) − attachment` |

The insured retains losses below attachment (the deductible) and losses above attachment + limit (uncovered excess). The market's obligation is exactly the net amount.

**Current simplification:** all policies use full-value coverage — `attachment = 0`, `limit = sum_insured`. Layer mechanics are fully implemented in `src/market.rs::on_insured_loss`; the attachment/limit parameters exist but are set to this degenerate case in canonical config.

**Panel splitting:** the net insured loss is pro-rated by each syndicate's share (in basis points). Each panel entry receives a separate `ClaimSettled` event. The sum of all `ClaimSettled` amounts equals the net insured loss, up to integer rounding no larger than the panel size. **[PARTIAL — current model has a single insurer per policy; panel splitting infrastructure exists but panel size = 1.]**

### §2.2 Annual policy terms and expiry `[ACTIVE]`

All policies are **annual contracts** written for a 12-month period, reflecting the Lloyd's standard placement cycle. Loss events can only trigger claims against policies that are currently active (inception ≤ event day < expiry). Syndicates re-underwrite their book at each renewal; premiums earned in one year do not carry forward.

**Expiry implementation:** `Market::expire_policies(year)` is called at the close of each `YearEnd` event, after `compute_year_stats` has captured the year's data. It removes all `bound_year == year` policies from the policies map and the peril-territory index.

**Aggregate annual GUL cap:** per (policy, year), cumulative GUL is capped at `sum_insured`. Tracked in `remaining_asset_value` in `src/market.rs`.

**Current simplification:** policies are treated as expiring at calendar year-end (`bound_year == year`). This avoids cross-year policy accounting while producing realistic annual statistics. The full quarterly-renewal model is described in §9.

---

## 3. Market Participants

### §3.1 Insureds `[ACTIVE]`

Each Insured owns one or more Assets and seeks insurance coverage each year. Insureds are active agents: they accept or reject quotes and accumulate GUL history. State: `id`, `risk` (asset description), `accepted_quotes` (pending binding), `total_ground_up_loss_by_year`. Source: `src/insured.rs`.

Canonical config: 90 small (sum_insured = 50M USD) + 10 large (sum_insured = 1B USD) = 100 insureds.

### §3.2 Insurers (Syndicates) `[ACTIVE]`

Each Insurer provides capacity and prices risks. State: `id`, `capital`, `rate`, `active_policies`. Capital is reset at each `YearStart`. Source: `src/insurer.rs`.

Canonical config: 5 insurers, 100B USD capital each, rate = 0.1 (10% of sum_insured).

### §3.3 Broker `[ACTIVE]`

A single Broker intermediates between Insureds and Insurers. Routes `CoverageRequested` to insurers via round-robin, assembles panel (currently single-insurer), and manages submission state. Source: `src/broker.rs`.

---

## 4. Pricing

### §4.1 Actuarial channel `[PLANNED]`

The actuarial channel produces a long-run expected loss cost estimate for a submitted risk. It is one of two inputs to the syndicate's final quote.

**Inputs:**
- Risk characteristics: line of business, sum insured, territory, coverage trigger, attachment/limit structure.
- Syndicate's accumulated loss experience for that line, encoded as an EWMA of historical loss ratios.
- Industry-benchmark loss ratio, published by the coordinator after each annual period (§8).

**Process:**
1. Apply line-of-business base loss cost from the syndicate's actuarial tables.
2. Blend own experience (EWMA loss ratio) with the industry benchmark using a credibility weight that increases with volume. Low-volume syndicates weight the benchmark heavily; specialists weight their own experience.
3. Apply risk-specific loadings: territory catastrophe factor, coverage trigger severity factor, attachment/limit adjustment.
4. Output: actuarial technical price (ATP) — the minimum premium at which the syndicate breaks even in expectation.

The ATP is not the quoted premium; it is a floor and an input.

*[TBD: EWMA decay parameter — per-line or per-syndicate?]*

**Current simplified implementation (`[PARTIAL]`):** `actuarial_price()` computes `expected_loss_fraction × sum_insured / target_loss_ratio` using a fixed per-insurer prior for `expected_loss_fraction` (calibrated from peril model parameters). The ATP is logged in `LeadQuoteIssued.atp`. `underwriter_premium()` returns `rate × sum_insured` independently (currently ≈ ATP). Both channels exist as separate code paths; neither yet uses runtime experience data.

```
ATP = E[annual_loss] / target_loss_ratio
```

where `E[annual_loss]` summed two peril contributions:
- Attritional: `annual_rate × exp(mu + σ²/2) × sum_insured` ≈ 0.164 × sum_insured
- Cat (WindstormAtlantic): `annual_frequency × (scale × shape / (shape − 1)) × sum_insured` ≈ 0.075 × sum_insured

Canonical values: `expected_loss_fraction = 0.239`, `target_loss_ratio = 0.70` → ATP rate ≈ 0.34. Source: `src/insurer.rs`.

### §4.2 Underwriter channel `[PLANNED]`

The underwriter channel reflects non-actuarial market intelligence: the current cycle position, relationship with the placing broker, and the observed lead quote (if any). It produces a market rate adjustment applied on top of the ATP.

**Inputs:**
- Current market cycle indicator (coordinator-published annually; derived from aggregate premium movement — see §8).
- Broker relationship score for the submitting broker.
- Lead quote (available only in follow-market mode; absent for the lead syndicate).
- Syndicate's risk appetite and cycle-sensitivity parameters.

**Process:**
1. **Cycle adjustment:** syndicates with high cycle sensitivity shade quotes toward the hard/soft market signal.
2. **Relationship adjustment:** a strong broker relationship reduces the loading for placement friction.
3. **Lead-follow adjustment (follow mode only):** follower syndicates observe the lead quote and allow it to pull their own price. A higher follow-weight parameter produces stronger herding.
4. Output: final quoted premium = `ATP × (1 + underwriter_adjustment)`.

**Capital constraint override:** before emitting the quote, the syndicate checks whether accepting the risk would breach exposure limits, concentration limits, or solvency floor. If so, it either declines or quotes a premium high enough to make acceptance capital-neutral. The coordinator does not intervene.

**Per-risk maximum line:** the syndicate checks whether the risk's limit exceeds its maximum single-risk loss tolerance (a fraction of initial capital). If so, it declines regardless of available capacity.

---

## 5. Placement `[PARTIAL]`

Lloyd's operates a subscription market: a lead syndicate sets terms, and followers subscribe on those terms (or decline). The quoting round is orchestrated by the coordinator.

### Full lead-follow sequence `[PLANNED]`

1. **Broker submission:** Broker selects a target panel and submits risk to the coordinator. Panel selection is driven by relationship scores and line specialism (see §8).
2. **Lead selection:** the coordinator identifies the lead syndicate — the panel member with the highest relationship score for that line. *[Alternative: broker nominates lead explicitly. TBD.]*
3. **Lead quote:** Lead syndicate receives the risk in lead mode (no prior quote visible). It runs both channels (§4.1, §4.2) and emits a `LeadQuoteIssued` event.
4. **Follow round:** remaining panel members receive the risk and the lead quote. Each runs both channels in follow mode.
5. **Panel assembly:** Broker collects quotes. If sufficient capacity is assembled, the risk is placed (`PolicyBound`).
6. **Shortfall handling:** if the panel is undersubscribed, the broker may approach additional syndicates (relationship-score ranked), or the risk is returned unplaced. *[TBD: retry limit, slip-down mechanics.]*

### Current implementation `[PARTIAL]`

Round-robin single-insurer routing (`src/broker.rs`). The quoting chain is:

```
CoverageRequested (+1d) → LeadQuoteRequested (same day) → LeadQuoteIssued (+1d) → QuotePresented (same day) → QuoteAccepted (+1d) → PolicyBound
```

Total `CoverageRequested` → `PolicyBound` cycle: **3 days**. The structural chain is complete; multi-syndicate panel assembly and lead/follow pricing modes are planned.

---

## 6. Loss Settlement `[ACTIVE]`

The full loss cascade from occurrence to capital deduction:

```
LossEvent (cat) or attritional schedule
  → InsuredLoss { policy_id, insured_id, peril, ground_up_loss }
    → Insured::on_insured_loss   (GUL accumulation)
    → Market::on_insured_loss    (apply policy terms)
      → ClaimSettled { policy_id, insurer_id, amount, peril }
        → Insurer::on_claim_settled   (capital -= amount)
```

### §6.1 Actuarial feedback `[PLANNED]`

Each loss updates the syndicate's accumulated loss experience and revises its actuarial estimate — the primary input to §4.1.

Syndicates learn from the full loss on a policy, not their proportional share. All syndicates on the same risk therefore converge toward the same long-run estimate regardless of line size. This is a structural rule.

### §6.2 Loss settlement invariants `[ACTIVE]`

The following invariants hold in every simulation run:

1. **GUL ≤ sum_insured** — damage fraction is clipped to [0, 1] before multiplication.
2. **Insured loss = 0 if GUL ≤ attachment** — below-deductible losses produce no `ClaimSettled`.
3. **Insured loss ≤ limit** — the policy cap is enforced in `Market::on_insured_loss`.
4. **Sum of `ClaimSettled` amounts = insured loss** — up to integer rounding ≤ panel size.
5. **Expired policies cannot generate claims** — removed from the peril-territory index at year-end before the next year's events are processed.

---

## 7. Capital and Solvency `[PLANNED]`

### §7.1 Syndicate entry `[PLANNED]`

After each annual review, the coordinator checks whether the industry combined ratio and current market premium rate index cross an entry-attractiveness threshold. If so, it creates one or more new Syndicate agents with parameters drawn from a calibrated distribution (risk appetite, specialism, initial capital). New syndicates receive low initial relationship scores with all brokers.

### §7.2 Exit via insolvency `[PLANNED]`

When a syndicate's capital falls below its solvency floor, the coordinator emits a `SyndicateInsolvent` event, removes it from active quoting, and transitions it to managed runoff (§7.3). Bound policies continue to their expiry; new submissions are declined. Capital depletion below the solvency floor also triggers this path from `on_claim_settled`.

### §7.3 Managed runoff and Central Fund `[TBD]`

**Managed runoff:** on insolvency, the coordinator transitions the syndicate to a runoff state. It accepts no new submissions but continues settling claims on bound policies until all have expired.

**Central Fund:** Lloyd's operates a mutual Central Fund funded by annual levies on all active syndicates. When an insolvent syndicate in runoff cannot meet a claim, the claim is paid from the Central Fund. The levy is a small annual deduction from each active syndicate's premium income.

**Design note:** the Central Fund is a welfare mechanism, not a cycle mechanism. It should not materially alter cycle period or amplitude.

### §7.4 Voluntary exit `[TBD]`

Whether to model voluntary capital withdrawal during soft markets is not yet decided.

---

## 8. Market Dynamics

### §8.1 Broker relationship score evolution `[PLANNED]`

Each broker maintains a relationship score per (syndicate, line-of-business) pair. Scores are initialised low for new relationships and evolve through placement activity.

**Update rules:**
- On successful placement (`RiskPlaced`): all participating syndicates receive a positive increment proportional to their share; the lead receives an additional increment.
- On quote declined (`QuoteDeclined`): small negative adjustment.
- On syndicate non-performance: larger negative adjustment.
- Passive decay: scores decay toward a baseline at a slow exponential rate.

**Score → routing behaviour:** when assembling a panel, the broker ranks syndicates by their score for the relevant line and selects the top-N. A score threshold filters out syndicates below minimum relationship quality. This produces placement stickiness (see `phenomena.md §5`).

**Initialisation:** new syndicates enter with a score draw from a low-mean distribution. They must win business (often at competitive prices) to build scores. *[TBD: whether an entering syndicate gets a one-time visibility boost from the coordinator to represent the real-world capital introduction process.]*

### §8.2 Annual coordinator statistics `[PLANNED]`

At the close of each annual period, the coordinator aggregates market-wide statistics and publishes them to all active syndicates. These are the primary outputs for benchmarking the simulation against Lloyd's empirical targets.

**Statistics published:**
- **Industry loss ratio:** total claims incurred divided by total premiums written. Feeds the cycle indicator consumed by the underwriter channel (§4.2).
- **Industry average premium rate:** market-wide average premium per unit of exposure, by line.
- **Aggregate claim frequency and severity:** the industry-benchmark component used in the §4.1 actuarial blend.
- **Active syndicate count and aggregate capacity:** signals how tight or loose market capacity is; an input to entry evaluation (§7.1).

**Design note:** statistics are a one-period-lagged signal — syndicates price for the coming year using the previous year's aggregate results. This lag is structural and contributes to cycle persistence.

**Central Fund levy:** *[TBD: whether to model explicitly.]* If Central Fund expenditure is tracked, an annual levy proportional to premium income is deducted from each active syndicate at this step.

---

## 9. Future Mechanics

### §9.1 Policy renewal seasonality `[PLANNED]`

Commercial insurance policies cluster at four standard renewal dates — **1 January, 1 April, 1 July, 1 October** — inherited from the historic quarter-day calendar.

### Approximate renewal weight by inception date (Lloyd's commercial property)

| Inception | Share | Primary drivers |
|---|---|---|
| 1 January | ~40% | Reinsurance, European corporate, international programmes |
| 1 April | ~20% | Japan, South Korea, Asia-Pacific, UK mid-market |
| 1 July | ~25% | US cat-exposed (Florida/SE wind), Australia, NZ, mid-year adjustments |
| 1 October | ~15% | Fiscal-year-driven accounts, US inland property, residual |

**Structural consequences:**
- A catastrophe striking in Q4 simultaneously hits active January-inception policies (last quarter) and active October-inception policies (first quarter). The aggregate exposure profile is not flat across the year.
- Concentration at January 1 gives that renewal round outsized market signalling power. Rate movements agreed in January propagate to April and July through the follow-pricing mechanism (§4.2).

*[TBD: implement per-policy inception and expiry dates. When implemented, the peril-territory index must be keyed by active-period rather than bound-year, and `expire_policies` must run continuously rather than only at year-end.]*

### §9.2 Programme structures and insurance towers `[PLANNED]`

When the required limit exceeds what a single panel placement can absorb, the risk is structured as a **programme** (tower): a vertical stack of consecutive layers, each covering a tranche of ground-up loss. Each layer is an independent contract with its own attachment, limit, premium, and panel.

**Layer economics — rate on line (ROL):** `ROL = premium / limit`. ROL decreases with attachment height. For a ground-up loss distribution F(x), the expected loss in the layer [A, A+L] is:

```
E[layer loss] = ∫_A^{A+L} (1 − F(x)) dx
```

As attachment A rises, the integrand shrinks — fewer events penetrate the layer. Expected loss per unit limit falls, and so must the ROL in a competitive market.

Typical ROL ranges (Lloyd's hard-market conditions, 2022–2024):

| Layer position | Description | ROL range |
|---|---|---|
| Primary / working layer | Low or zero attachment; hit by attritional and cat | 15–30% |
| First excess | Attaches above working layer; rarely hit by attritionals | 5–15% |
| Upper / remote | High attachment; cat events only | 1–8% |

### Threshold heuristics (calibration estimates)

| Insured's sum insured | Typical programme structure |
|---|---|
| < £30M | Single-layer policy |
| £30M – £100M | 2-layer programme: primary + 1 excess layer |
| £100M – £400M | 3–5 layer programme |
| £400M – £1B | 5–8 layers; may include ILS or cat bond capacity at upper layers |
| > £1B | 8+ layers; Lloyd's typically leads on lower layers |

**Layer-position specialism:** syndicates that prefer high-frequency, lower-severity exposure concentrate in primary layers; those seeking low-frequency, high-severity tail risk concentrate in upper layers. This creates a layer-position specialism dimension orthogonal to line-of-business specialism.

### §9.3 Experience rating `[PLANNED]`

Per-insured surcharge mechanism: an insured with above-market loss history attracts a higher renewal premium from the actuarial channel. Closes the loss-experience feedback loop at the individual insured level rather than only at the portfolio level.
