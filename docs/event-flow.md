# Event Flow Diagram

End-to-end flow of all event types through the discrete-event simulation.
Time advances by pulling the lowest-timestamp event from a min-heap priority queue.

```mermaid
flowchart TD
    %% ── Year lifecycle ──────────────────────────────────────────────────────

    SS["**SimulationStart**\n{year_start}"]
    YE["**YearEnd**\n{year}"]
    LE["**LossEvent**\n{event_id, region, peril}"]
    SS_NEXT["**SimulationStart**\n{year_start: N+1}"]

    SS -->|"Broker.generate_submissions()\n→ spread over first 180 days"| SA
    SS -->|"perils::schedule_loss_events\nPoisson(λ) per PerilConfig\n(cat perils only)"| LE
    SS -->|"schedule day 365"| YE

    %% ── Quoting round ───────────────────────────────────────────────────────

    subgraph Broker
        SA["**SubmissionArrived**\n{submission_id, broker_id, insured_id, risk}"]
    end

    subgraph Market["Market (Coordinator)"]
        QR_L["**QuoteRequested**\n{is_lead: true}\n+2 days"]
        QR_F["**QuoteRequested**\n{is_lead: false}\n+3 days"]
        PB["**PolicyBound**\n{submission_id, panel}\n+5 days"]
        ABANDON_EVENT["**SubmissionAbandoned**\n{submission_id}\n(log only)"]
        LE_D["on_loss_event\nmatches territory + peril\nsamples damage_fraction per policy\n→ ground_up_loss = df × sum_insured"]
        IL_D["on_insured_loss\napplies policy terms\ngross = min(ground_up, limit)\nnet = gross − attachment"]
        STATS["compute_year_stats\n→ industry_loss_ratio\n→ YTD reset\n→ expire year policies"]
    end

    subgraph Syndicate["Syndicate  (ATP pricing)"]
        QI_L["**QuoteIssued**\n{is_lead: true, premium}\nATP blended with benchmark"]
        QD_L["**QuoteDeclined**\n{submission_id, syndicate_id}"]
        QI_F["**QuoteIssued**\n{is_lead: false, premium}\nfollower pricing vs lead"]
        QD_F["**QuoteDeclined**\n{submission_id, syndicate_id}"]
        CS_S["on_claim_settled\ncapital −= amount\n→ true if capital < solvency floor"]
        YE_S["on_year_end\nEWMA ← realised loss ratio\n(per line of business)"]
    end

    subgraph Insured["Insured"]
        INS_H["on_insured_loss\naccumulate total_ground_up_loss_by_year"]
    end

    SA --> QR_L
    QR_L --> QI_L
    QR_L --> QD_L
    QI_L -->|"record lead premium\ninvite all other syndicates"| QR_F
    QD_L -->|"lead declined → emit SubmissionAbandoned\n(no followers invited)"| ABANDON_EVENT
    QR_F --> QI_F
    QR_F --> QD_F
    QI_F -->|"all followers responded"| PB
    QD_F -->|"all followers responded"| PB

    %% ── Loss cascade ────────────────────────────────────────────────────────

    IL["**InsuredLoss**\n{policy_id, insured_id, peril, ground_up_loss}"]
    CS["**ClaimSettled**\n{policy_id, syndicate_id, amount}"]
    ATTR["perils::schedule_attritional_claims_for_policy\nPoisson(λ/territory) per-policy\nsamples damage_fraction × sum_insured\nspread across year"]

    LE --> LE_D
    LE_D -->|"one InsuredLoss per matching policy\n(cat + non-attritional perils only)"| IL
    IL --> INS_H
    IL --> IL_D
    IL_D -->|"one ClaimSettled\nper panel entry\n(net > 0 only)"| CS
    PB -->|"risk covers Attritional?"| ATTR
    ATTR -->|"one InsuredLoss\nper occurrence"| IL
    CS --> CS_S
    CS -->|"accumulate ytd_claims_by_line"| STATS

    PB -->|"accumulate ytd_premiums_by_line"| STATS

    %% ── Year end ────────────────────────────────────────────────────────────

    YE --> STATS
    STATS -->|"publish current_industry_benchmark\nfor next year's ATP"| YE_S
    YE -->|"schedule SimulationStart(N+1)"| SS_NEXT

    %% ── Entry / insolvency ──────────────────────────────────────────────────

    SE(["**SyndicateEntered**\n{syndicate_id}\n(handler: no-op)"])
    SI["**SyndicateInsolvency**\n{syndicate_id}\nsets syndicate.is_active = false"]

    CS -->|"capital < solvency floor\n(20% of initial_capital)"| SI
```

## Legend

| Shape | Meaning |
|-------|---------|
| Rectangle | Active event type — fires and produces downstream events |
| Rounded rectangle | Terminal state — no further events produced |
| Label on arrow | Side-effect or scheduling condition |

## Event index

| # | Event | Producer | Consumer |
|---|-------|----------|----------|
| 1 | `SimulationStart` | `handle_year_end` / external seed | `Simulation::handle_simulation_start` |
| 2 | `YearEnd` | `handle_simulation_start` | `Simulation::dispatch` → `Market::compute_year_stats`, `Syndicate::on_year_end`, `Broker::on_year_end`, `Market::expire_policies` |
| 3 | `SubmissionArrived` {submission_id, broker_id, insured_id, risk} | `Broker::generate_submissions` | `Market::on_submission_arrived` |
| 4 | `QuoteRequested` | `Market::on_submission_arrived` (+2 days), `Market::on_lead_quote_issued` (+3 days) | `Syndicate::on_quote_requested` |
| 5 | `QuoteIssued` | `Syndicate::on_quote_requested` | `Market::on_lead_quote_issued` / `Market::on_follower_quote_issued` |
| 6 | `QuoteDeclined` | `Syndicate::on_quote_requested` | `Market::on_quote_declined` |
| 7 | `SubmissionAbandoned` | `Market::on_quote_declined` (when lead declines) | none (log only) |
| 8 | `PolicyBound` | `Market::assemble_panel` (+5 days from last follower response) | `Market::on_policy_bound` (registers policy, YTD premium); `perils::schedule_attritional_claims_for_policy` (if risk covers Attritional, emits `InsuredLoss`) |
| 9 | `LossEvent` | `handle_simulation_start` via `perils::schedule_loss_events` (Poisson frequency, **cat perils only**; no severity field) | `Market::on_loss_event` → samples `damage_fraction` per policy → emits `InsuredLoss` |
| 10 | `InsuredLoss` {policy_id, insured_id, peril, ground_up_loss} | `Market::on_loss_event` (cat perils) or `perils::schedule_attritional_claims_for_policy` (Attritional, per-policy) | `Insured::on_insured_loss` (accumulate stats) + `Market::on_insured_loss` → applies policy terms → emits `ClaimSettled` |
| 11 | `ClaimSettled` | `Market::on_insured_loss` | `Syndicate::on_claim_settled` + `Market::on_claim_settled` (YTD) |
| 12 | `SyndicateEntered` | — | no-op |
| 13 | `SyndicateInsolvency` | `Syndicate::on_claim_settled` (when capital < solvency floor) | `Simulation::dispatch` → sets `syndicate.is_active = false` |

## Day offsets

- `SubmissionArrived` → `QuoteRequested` (lead): **+2 days**
- Lead `QuoteIssued` → `QuoteRequested` (followers): **+3 days**
- Last follower response → `PolicyBound`: **+5 days**
- Total submission-to-bind cycle: **~10 days**
- `SubmissionArrived` spread: **first 180 days** of each year (~4–5 submissions/day)
- `LossEvent` → `InsuredLoss` → `ClaimSettled`: **same day** (no offset)
- Attritional `InsuredLoss`: spread across year days (Poisson per-policy)
- `SimulationStart` → `YearEnd`: **day 365 of that year**
- `YearEnd` → next `SimulationStart`: **day 1 of next year**

## Damage fraction model

`LossEvent` no longer carries a `severity` field. Instead, when a `LossEvent` fires,
`Market::on_loss_event` samples a **damage fraction** from `DamageFractionModel` for
each matching policy:

```
ground_up_loss = damage_fraction × sum_insured   (naturally ≤ sum_insured)
```

The damage fraction is drawn from per-peril `DamageFractionModel` distributions
(LogNormal or Pareto, clipped to [0.0, 1.0]). Policy terms are applied in
`Market::on_insured_loss`:

```
gross = min(ground_up_loss, limit)
net   = gross − attachment
→ ClaimSettled per panel entry  (if net > 0)
```

Attritional claims follow the same path: `schedule_attritional_claims_for_policy`
emits `InsuredLoss` events (not `ClaimSettled` directly).
