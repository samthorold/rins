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
| Actuarial channel (ATP pricing + EWMA experience update) | ACTIVE | `src/insurer.rs::actuarial_price`, `on_year_end` |
| Separate cat / attritional ELF (cat ELF anchored, attritional EWMA-updated) | ACTIVE | `src/insurer.rs::on_year_end` |
| Profit loading above ATP in underwriter channel | ACTIVE | `src/insurer.rs::underwriter_premium` |
| Expense loading (net premium credited to capital) | PARTIAL — `expense_ratio` applied at bind; explicit brokerage not modelled | `src/insurer.rs::on_policy_bound` |
| Exposure management (per-risk line size, cat aggregate PML constraint) | ACTIVE — capital-linked fractions (net_line_capacity=0.30, solvency_capital_fraction=0.30, pml_200 derived from cat model) | `src/insurer.rs::on_lead_quote_requested`, `§4.4` |
| Lead-follow quoting (round-robin + decline re-routing) | ACTIVE (PARTIAL — single-insurer panel; no follow-market mode) | `src/broker.rs` |
| Underwriter channel / AP/TP ratio (MS3 AvT) | ACTIVE — three-level pricing: ATP → TP (× profit loading) → AP (× market_ap_tp_factor); factor driven by trailing 3yr CR + capacity pressure | `src/insurer.rs::underwriter_premium`, `src/simulation.rs::handle_year_end` |
| Supply / demand balance (insured reservation price) | ACTIVE — step-function demand curve; inelastic at current 6–8% rates; Dropped# is supply-constrained not demand-constrained | `src/insured.rs::on_quote_presented` |
| Broker relationship scores | PLANNED | — |
| Syndicate entry / exit (capital entry) | ACTIVE — AP/TP > 1.10 trigger + new insurer spawn; 1-year cooldown; critical for underwriting cycle emergence | `src/simulation.rs::handle_year_end` |
| Annual coordinator statistics | PLANNED | — |
| Quarterly renewal seasonality | PLANNED | — |
| Programme structures / towers | PLANNED | — |
| Experience rating (per-insured surcharge) | PLANNED | — |
| Persistent capital (premiums accumulate, claims erode, no annual reset) | ACTIVE | `src/insurer.rs` |
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

Each Insured owns one or more Assets and seeks insurance coverage each year. Insureds are active agents: they evaluate quotes against a **reservation price** and accumulate GUL history. State: `id`, `risk` (asset description), `max_rate_on_line`. Source: `src/insured.rs`.

**Reservation price:** each insured has a maximum acceptable rate on line (`max_rate_on_line`). `Insured::on_quote_presented` computes `rate = premium / sum_insured`; if `rate > max_rate_on_line` it emits `QuoteRejected` instead of `QuoteAccepted`. A rejected insured is uninsured for the year but retries at the next annual renewal (same timing offset as an accepted quote — the renewal `CoverageRequested` fires `361 − 3 = 358` days after the rejection day). Canonical value: **0.15** (15% RoL — above the typical 6–8% rate band, so rarely binding at current pricing; lowers as capital-linked pricing is introduced).

**Demand curve structure:** the reservation price creates a step-function demand curve — inelastic until the price ceiling is hit, then vertically zero. At canonical rates of 6–8%, demand is near-fully inelastic: every insured that can find a willing insurer will buy. The `Dropped#` column in the year table therefore measures supply-side capacity exhaustion, not demand-side price sensitivity — insureds are willing buyers shut out by capital-constrained insurers.

This is a reasonable first-order model for Lloyd's *primary* commercial lines (marine, property, energy), which are largely mandatory or balance-sheet driven and for which demand is genuinely inelastic across the historical rate range. It is less appropriate for upper excess-of-loss layers, where buyers make explicit cost-benefit decisions about each additional layer and will drop remote layers when ROLs spike — a demand-side behaviour that would reduce the pressure on `Dropped#` in hard markets. Heterogeneous reservation prices by layer position would capture this, and is a future extension aligned with phenomenon 10 (Layer-Position Premium Gradient).

Canonical config: 100 uniform insureds (sum_insured = 50M USD each).

### §3.2 Insurers (Syndicates) `[ACTIVE]`

Each Insurer provides capacity and prices risks. State: `id`, `capital`, `insolvent`, `active_policies`. Capital is endowed once at construction and evolves with the insurer's P&L (premiums credited at bind, claims deducted at settlement). No annual re-endowment. Capital floors at zero; once exhausted the insurer is marked insolvent and declines all new quotes. Source: `src/insurer.rs`.

Canonical config: 5 insurers, 1B USD initial capital each.

### §3.3 Broker `[ACTIVE]`

A single Broker intermediates between Insureds and Insurers. Routes `CoverageRequested` to insurers via round-robin, assembles panel (currently single-insurer), and manages submission state. Source: `src/broker.rs`.

**All-declined path:** when every solicited insurer declines a submission (`quotes_outstanding` reaches zero with `best_quote = None`), the broker emits `SubmissionDropped { submission_id, insured_id }` instead of silently dropping the submission. The simulation dispatcher handles `SubmissionDropped` identically to `QuoteRejected`: it schedules a renewal `CoverageRequested` at day + 358, so the insured retries next year rather than permanently vanishing from the model.

---

## 4. Pricing

### §4.1 Actuarial channel `[PARTIAL]`

The actuarial channel produces a long-run expected loss cost estimate for a submitted risk. It is one of two inputs to the syndicate's final quote.

**Inputs:**
- Risk characteristics: line of business, sum insured, territory, coverage trigger, attachment/limit structure.
- Syndicate's accumulated loss experience for that line, encoded as an EWMA of historical loss ratios.
- Industry-benchmark loss ratio, published by the coordinator after each annual period (§8).

**Process:**
1. Apply line-of-business base loss cost from the syndicate's actuarial tables.
2. Blend own experience (EWMA loss ratio) with the industry benchmark using a credibility weight that increases with volume. Low-volume syndicates weight the benchmark heavily; specialists weight their own experience.
3. Apply risk-specific loadings: territory catastrophe factor, coverage trigger severity factor, attachment/limit adjustment.
4. Output: actuarial technical price (ATP) — the technically priced premium at which the syndicate expects to achieve its `target_loss_ratio`. With `target_loss_ratio < 1` the ATP exceeds expected loss by a built-in profit margin (`ATP = E[loss] / target_loss_ratio`). ATP is the technical premium, not merely a break-even floor.

In Step 0 (technical pricing baseline), ATP is the quoted premium. The underwriter channel [§4.2] applies a multiplicative adjustment on top of ATP; with no underwriter signal active the adjustment is 1.0 and `premium = ATP`.

*[TBD: EWMA decay parameter — per-line or per-syndicate?]*

**Current simplified implementation:** `actuarial_price()` computes `(attritional_elf + cat_elf) × sum_insured / target_loss_ratio`.

The expected loss fraction is split into two components that are updated by different mechanisms — reflecting standard actuarial practice and Lloyd's market convention:

**Attritional ELF** (`attritional_elf`): updated each `YearEnd` via EWMA from realized attritional burning cost:

```
new_att_elf = α × (year_attritional_claims / year_exposure) + (1 − α) × old_att_elf
```

where `α = ewma_credibility` (canonical 0.3). Attritional losses are high-frequency: a single year provides useful data, and the EWMA update is credible. Only attritional `ClaimSettled` events (peril = `Attritional`) contribute to `year_attritional_claims`.

**Cat ELF** (`cat_elf`): **anchored — never updated from experience.** The initial `cat_elf` is derived from a cat model (Poisson frequency × expected Pareto damage fraction) and held fixed throughout the simulation. This mirrors real-world practice: vendor cat models (RMS, AIR, Verisk) produce an Expected Annual Loss (EAL/AAL) estimate that is treated as stable. A decade without a hurricane is *not* evidence that hurricanes have become rarer — it is a benign sample from the same distribution. Updating cat ELF via EWMA from experience would cause systematic rate softening after quiet periods, which is the dominant failure mode in soft-market cycles.

**Why separation matters in Lloyd's:** Lloyd's Minimum Standards MS3 (*Price and Rate Monitoring*, 2021) requires syndicates to track the ratio of **Actual Premium to Technical Premium** (AvT). The Technical Premium is the actuarially required floor; it must include modelled cat loading derived from the cat model, not from recent experience. When AvT < 1.0, the syndicate is pricing below technical and must justify the shortfall. The MS3 framework was strengthened in 2022 with hard floor requirements and retrospective testing — specifically to prevent experience-rated cat ELF erosion during benign periods.

```
ATP = (attritional_elf + cat_elf) × sum_insured / target_loss_ratio
```

where:
- Attritional: `annual_rate × exp(mu + σ²/2) × sum_insured` → canonical att_elf ≈ 3.0%
- Cat (WindstormAtlantic): `annual_frequency × (scale × shape / (shape − 1)) × sum_insured` → canonical cat_elf ≈ 1.5%

Canonical values: `attritional_elf = 0.030`, `cat_elf = 0.015`, total ELF = 0.045, `target_loss_ratio = 0.55` → ATP rate ≈ 8.2%. Source: `src/insurer.rs`.

### §4.3 Expense loading and broker fees `[PARTIAL]`

The premium charged to an insured must recover not just expected claims but also the syndicate's acquisition costs, management overheads, Lloyd's levies, and cost of capital. Expenses are expressed as a percentage of **gross written premium (GWP)**, making the loading formula multiplicative, not additive:

```
Gross Premium = Pure Risk Premium / (1 − Expense Ratio)
```

Using an additive form (gross = pure + load × pure) systematically underprices, because each unit of loading must itself be covered by that loading. If the pure premium is £60 and the total expense ratio is 40%, the gross premium is £60 / 0.60 = £100, not £84.

#### Brokerage

Brokerage is the broker's fee for placing the risk. It is embedded in the gross premium — the insured pays one number; the Lloyd's broker deducts brokerage and remits the remainder to the Premium Trust Fund. The syndicate's accounts then record the **full gross premium as GWP**; brokerage appears as an **acquisition cost on the expense side** of the technical account, not as a deduction from revenue. The brokerage rate is agreed between broker and syndicate and documented on the Market Reform Contract (MRC).

**Typical brokerage rates:**

| Channel | Rate |
|---------|------|
| XoL reinsurance (open market) | ~15% of gross premium |
| Direct specialty (open market) | 10–20% of gross premium |
| Facultative reinsurance | 5–10% |
| Large treaty reinsurance | 1–5% |
| Coverholder / binding authority (ceding commission) | 25–35% of gross premium |

Approximately 45% of Lloyd's premium is written through coverholders (binding authorities), which carry higher total acquisition costs because a ceding commission, a Lloyd's broker fee, and the managing agent's overhead are all levied on the same premium.

#### Expense loading components

| Component | Lloyd's market (2024) |
|-----------|----------------------|
| Brokerage and other acquisition costs | ~22.6% of NEP |
| Management expenses | ~11.8% of NEP |
| **Total operating expense ratio** | **~34.4% of NEP** |
| Annual subscription (Lloyd's levy) | 0.36% of GWP |
| Central Fund contribution | 0.35% of GWP |

**Net Earned Premium (NEP)** — GWP minus outward reinsurance ceded, adjusted for unearned premium movements — is the denominator for all Lloyd's ratio calculations. The combined ratio is `loss_ratio + expense_ratio`; at 86.9% (2024) with a 34.4% expense ratio, the implied loss ratio is ~52.5%.

#### Profit / cost of capital loading

The cost of capital loading is the return the syndicate requires to justify allocating capital to a risk. Lloyd's Minimum Standards MS3 requires this loading to be included in the technical price and grounded in actual capital allocation. Representative benchmarks:

- Primary / working layers: ~5% cost of capital loading
- Upper / cat-exposed excess layers: up to ~30%

The higher loading for upper layers reflects their greater capital consumption — they carry rare but catastrophic losses that require substantially more risk capital per unit of limit.

#### The premium flow

```
Insured pays Gross Premium to Lloyd's Broker
  → Broker deducts Brokerage (~10–20%)
    → Net-of-brokerage enters Premium Trust Fund (ringfenced for claims)
      → Distributed to Syndicates pro-rata by line
        → Syndicate books GWP; brokerage appears as acquisition cost
          → Syndicate pays outward RI premium (RI broker takes ~15%)
```

For coverholder business: coverholder collects premium, deducts ceding commission (25–35%), remits net to managing agent via Lloyd's broker, who also takes a fee. Multiple deduction stages produce the highest total acquisition cost structure.

#### Implementation note

**Current implementation (`[PARTIAL]`):** `Insurer::on_policy_bound` credits `net_premium = gross_premium × (1 − expense_ratio)` to capital. Canonical `expense_ratio = 0.344` (Lloyd's 2024: 22.6% acquisition + 11.8% management). The deduction is applied at bind time, not at earning — a simplification that is acceptable for annual contracts but would need adjustment if multi-year or mid-year cancellation were introduced.

What is not yet modelled:
- Brokerage as a separate cash flow (currently folded into `expense_ratio`).
- The correct pricing formula is `ATP = E[loss] / (1 − expense_ratio − profit_margin)`; the current formula uses `target_loss_ratio` as a single divisor, which conflates the profit margin with the expense loading. When separated:
  - With `expense_ratio = 0.344` and a target profit margin of ~10%, `target_loss_ratio ≈ 1 − 0.344 − 0.10 = 0.556`, close to the current canonical 0.55.
- Outward reinsurance premiums and the distinction between GWP and NEP.

---

### §4.2 Underwriter channel `[ACTIVE]`

The underwriter channel reflects non-actuarial market intelligence: the current cycle position, relationship with the placing broker, and the observed lead quote (if any). It produces a multiplicative adjustment applied to the ATP: `premium = AP = TP × market_ap_tp_factor`.

**Three-level pricing model (MS3 AvT framework):**

```
ATP  = (attritional_elf + cat_elf) × sum_insured / target_loss_ratio
TP   = ATP × (1 + profit_loading)         — Technical Premium: long-run actuarial floor
AP   = TP × market_ap_tp_factor           — Actual Premium: market-clearing price
```

`market_ap_tp_factor` (the AP/TP ratio, equivalent to MS3's "AvT" — Actual vs Technical) is a coordinator field published annually. It is computed at each `YearEnd` from trailing combined ratios and capacity pressure:

```
cr_signal       = clamp(avg_3yr_CR − 1.0,  −0.25,  0.40)
capacity_uplift = 0.05  if year_dropped_count > 10, else 0.0
factor          = clamp(1.0 + cr_signal + capacity_uplift,  0.90,  1.40)
```

Factor semantics: 0.90 = soft floor (AP = 90% of TP); 1.00 = break-even; 1.40 = hard cap (AP = 140% of TP). Insufficient history (< 2 years) defaults to 1.0 (neutral, for warmup). MS3 tracks AvT as a regulatory signal: a persistent AvT < 1.0 indicates the market is pricing below technical and flags syndicate-level intervention risk.

**Calibration note:** `cat_elf` is anchored — not updated from experience — so TP does not erode during quiet cat periods. The AP/TP mechanism therefore produces rate softening through the market factor, not through technical price drift. This mirrors the MS3 Technical Rate / Actual vs Technical (AvT) distinction.

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

**Exposure limit enforcement `[ACTIVE]`:** before emitting the quote, the syndicate checks its exposure limits and emits `LeadQuoteDeclined` if either is breached. The broker then re-routes to the next insurer (round-robin, up to N attempts). Both limits are capital-linked and recalculated from current capital at quote time — see §4.4.

---

## 4.4 Exposure Management `[ACTIVE]`

Syndicates manage two distinct exposure limits, both grounded in Lloyd's Franchise Guidelines (Market Bulletin Y5375, 2022):

### Per-risk line size

**Lloyd's rule:** the maximum net line (the syndicate's retained exposure after outward reinsurance) on any single risk must not exceed **30% of ECA plus profit**. The gross line must not exceed **min(25% of GWP, £200M)**.

Since ECA ≈ Funds at Lloyd's ≈ the syndicate's available capital, this is a **capital-linked constraint**:

```
max_net_line = net_line_capacity × capital
```

where `net_line_capacity ≈ 0.30` is the fraction of capital the syndicate may commit to a single risk. As capital depletes (post-loss), the maximum line shrinks proportionally.

The gross line cap ties a second independent constraint to annual premium income — a syndicate that loses significant capital cannot simply write its way to a large line in one year.

### Cat aggregate per peril zone

**Lloyd's mechanism:** the SCR (Solvency Capital Requirement) is set at the 99.5th percentile (1-in-200 year) of total claims on an ultimate basis. The ECA = SCR × 1.35, providing a 35% buffer above the solvency minimum. A syndicate's cat aggregate — the sum of sum-insured across all active policies in a correlated peril zone — is bounded by the requirement that the 1-in-200 scenario loss is coverable within the ECA:

```
PML_200 = cat_aggregate × pml_damage_fraction_200
PML_200 ≤ solvency_capital_fraction × capital
```

where:
- `pml_damage_fraction_200` — the damage fraction at the 1-in-200 return period, derived from the cat model distribution (for Pareto(scale, shape) at annual frequency λ: `scale × (200 × λ)^(1/shape)`).
- `solvency_capital_fraction` — the fraction of capital the syndicate allocates to covering the 1-in-200 cat scenario (reflects how much of the ECA the cat book consumes vs. attritional and operational risks). Not a bright regulatory line; a calibration parameter.

Rearranging to a capacity limit:

```
max_cat_aggregate = solvency_capital_fraction × capital / pml_damage_fraction_200
```

Both parameters depend on the cat model and the syndicate's portfolio composition. As capital depletes post-loss, `max_cat_aggregate` tightens proportionally — this is the mechanism that produces capacity crunches after catastrophe events. A syndicate that loses 40% of its capital after a major cat year can write only 60% of its previous cat aggregate, even before any price increase. The capacity crunch and the price increase are both consequences of the same capital depletion; they reinforce each other.

**Lloyd's tail risk constraint** (Y5375): the 1-in-500 loss must not exceed 135% × 1-in-200 loss. This prevents syndicates from building books with thin tails at the 1-in-200 and cliff-edge exposure beyond it. It is a shape constraint on the aggregate cat loss distribution, not an additional aggregate cap.

### Gross vs. net

Lloyd's distinguishes gross (pre-reinsurance) and net (post-reinsurance) exposure. The primary regulatory constraints apply to **net** figures. Syndicates can write larger gross lines than the 30% ECA limit allows, provided they cede enough outward reinsurance to bring the net below the limit. In the current model there is no outward reinsurance, so gross = net throughout.

### Current implementation `[ACTIVE]`

Both limits are enforced as **capital-linked fractions** recalculated from `self.capital` at every quote:

```rust
// Per-risk line size
effective_line_limit = net_line_capacity × capital          // e.g. 0.30 × 500M = 150M USD

// Cat aggregate
effective_cat_limit  = solvency_capital_fraction × capital / pml_damage_fraction_200
                     // e.g. 0.30 × 500M / 0.252 ≈ 595M USD
```

`pml_damage_fraction_200` is derived once in `Simulation::from_config()` from the cat model:

```
pml_damage_fraction_200 = pareto_scale × (200 × annual_frequency)^(1 / pareto_shape)
```

For canonical Pareto(scale=0.04, shape=2.5, λ=0.5): `0.04 × 100^0.4 ≈ 0.252`.

**Config fields** (`src/config.rs`, `InsurerConfig`):
- `net_line_capacity: Option<f64>` — canonical `Some(0.30)`; `None` = unlimited (tests only).
- `solvency_capital_fraction: Option<f64>` — canonical `Some(0.30)`; `None` = unlimited (tests only).

The hard-decline at limit is realistic — Lloyd's Franchise Guidelines are regulatory hard floors requiring a dispensation to exceed. As capital is depleted post-loss, both limits tighten proportionally; as premiums accumulate, they relax. This is the feedback loop that produces post-catastrophe capacity crunches and the subsequent premium hardening.

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

Round-robin single-insurer routing with exposure-limit re-routing (`src/broker.rs`). The quoting chain is:

```
CoverageRequested (+1d) → LeadQuoteRequested (same day) → LeadQuoteIssued (+1d) → QuotePresented (same day) → QuoteAccepted (+1d) → PolicyBound
```

If the selected insurer breaches an exposure limit it emits `LeadQuoteDeclined` instead of `LeadQuoteIssued`. The broker re-routes to the next insurer in the round-robin (up to N attempts); if all N insurers decline, the submission is dropped silently.

Total `CoverageRequested` → `PolicyBound` cycle: **3 days** (on the happy path). Multi-syndicate panel assembly and lead/follow pricing modes are planned.

---

## 6. Loss Settlement `[ACTIVE]`

The full loss cascade from occurrence to capital deduction:

```
LossEvent (cat) or attritional schedule
  → InsuredLoss { policy_id, insured_id, peril, ground_up_loss }
    → Insured::on_insured_loss   (GUL accumulation)
    → Market::on_insured_loss    (apply policy terms)
      → ClaimSettled { policy_id, insurer_id, amount, peril }
        → Insurer::on_claim_settled   (pays min(amount, capital), floors capital at 0; emits InsurerInsolvent on first crossing zero)
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

## 7. Capital and Solvency

### §7.0 Persistent capital model `[ACTIVE]`

Each insurer begins with `initial_capital` (canonical: 1 B USD). Capital evolves throughout
the simulation:

- **Credit:** net premium (gross premium × (1 − expense_ratio)) credited at `PolicyBound`.
- **Debit:** settled claim amount deducted at `ClaimSettled`.
- **Carry-over:** capital at year-end becomes the opening balance for the next year. There is
  no annual re-endowment.

**Lloyd's context — Funds at Lloyd's (FAL):** Members lodge capital with Lloyd's as security
for their underwriting. FAL minimum is 40% of stamp capacity (maximum NPI), with an additional
35% Economic Capital Assessment uplift (ECA = SCR × 1.35) applied by Lloyd's above the
Solvency II minimum. The market-wide target solvency ratio is ≥ 140% (Lloyd's actual 2024: 206%).

**Capital as the underwriting constraint:** a syndicate's capital directly governs its maximum
exposure through two mechanisms (§4.4): the per-risk net line limit (30% of ECA) and the cat
aggregate limit (PML at 1-in-200 return period ≤ a fraction of ECA). Capital depletion therefore
compresses both limits simultaneously — a syndicate that has absorbed large losses writes smaller
lines and less cat aggregate, reducing its capacity contribution to the market. This feedback
is the primary engine of post-catastrophe capacity crunches.

**Calibration:** With canonical parameters (100 insureds, 5 insurers, total ELF ≈ 6.3%,
target_LR = 0.80, SI = 50 M USD, profit_loading = 5%), each insurer writes ≈ 105 policies × 50 M USD
at ~8.3% premium rate ≈ 435 M USD GWP per year. Initial capital of 500 M USD represents ≈ 115%
of NPI — below the Lloyd's MWSCR target of 140–206% by design, to make solvency events
meaningful within a 20-year simulation horizon. The per-risk net line limit of `0.30 × 500M = 150M USD`
exceeds any single policy's sum_insured (50M USD), so per-risk limits are non-binding in the
current single-territory model; the cat aggregate limit is the operative constraint.

Source: `src/insurer.rs`, `src/config.rs`.

### §7.1 Syndicate entry `[ACTIVE]`

After each annual review (`handle_year_end`), the coordinator checks whether the market AP/TP ratio crosses an entry-attractiveness threshold. If so, it creates a new Insurer agent with parameters drawn from the config template. New insurers enter the broker's round-robin immediately.

**Entry trigger:** `market_ap_tp_factor > 1.10` — market pricing more than 10% above technical premium. The AP/TP factor already encodes the full loss-history signal (trailing 3yr combined ratio) and capacity pressure; no separate CR guard is needed. A factor above threshold implies expected profitability exceeds cost of capital, which is the empirically observed mechanism for capital formation.

**Cooldown:** 1 year — reflects Lloyd's regulatory formation and approval timeline (12–18 months). `last_entry_year` is updated at each spawn; subsequent years check `year − last_entry_year ≥ 1`.

**Warmup guard:** entry is suppressed during warmup years (`year.0 ≤ warmup_years`), when `market_ap_tp_factor = 1.0` (neutral, insufficient history).

**Capital sources in practice:** new Lloyd's capacity enters via Names capital top-up, corporate member capital injection, PE-backed managing agency formation, or ILS/sidecar structures collateralised against a specific underwriting year. The simulation represents all of these as a single `InsurerEntered` event that creates an insurer agent with initial capital from the config template.

**Timing lag:** historical pattern (Bermuda class of 1993 post-Andrew; class of 2001 post-9/11; class of 2006 post-Katrina) — meaningful new capacity emerged 12–18 months after the triggering event. The 1-year cooldown approximates this formation lag.

**Relationship-building lag:** a new entrant starts with no broker relationship scores and enters the round-robin at the back. This second lag means new capacity contributes incrementally and reaches full participation only after 2–3 years of active placement. The combination of the two lags — formation + relationship-building — sustains elevated rates for several years after the shock, which is the empirically observed hard-market duration.

**Implementation:** `src/simulation.rs::handle_year_end` → `spawn_new_insurer`. 1-in-3 new entrants are aggressive (optimistic internal cat model; `pml_damage_fraction_override = Some(0.126)`). `InsurerEntered { insurer_id, initial_capital, is_aggressive }` is logged directly. Voluntary exit during soft markets (§7.4) would close the lower tail of the cycle.

### §7.2 Exit via insolvency `[ACTIVE (PARTIAL)]`

When an insurer's capital is exhausted by a claim, `on_claim_settled` floors capital at zero,
sets `insolvent = true`, and returns an `InsurerInsolvent` event (logged same day as the
triggering `ClaimSettled`). The coordinator's `InsurerInsolvent` dispatch is a no-op — the
state change lives in the insurer aggregate. From that point on, `on_lead_quote_requested`
returns `LeadQuoteDeclined { reason: Insolvent }`, causing the broker to re-route to another
insurer. Existing in-force policies continue in run-off; future claims are paid down to capital = 0.
Central Fund and managed runoff remain `[TBD]` (§7.3).

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
