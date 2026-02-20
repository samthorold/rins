# Event Flow Diagram

End-to-end flow of all event types through the discrete-event simulation.
Time advances by pulling the lowest-timestamp event from a min-heap priority queue.

```mermaid
flowchart TD
    %% ── Year lifecycle ──────────────────────────────────────────────────────

    SS["**SimulationStart**\n{year_start}"]
    YE["**YearEnd**\n{year}"]
    LE["**LossEvent**\n{event_id, region, peril, severity}"]
    SS_NEXT["**SimulationStart**\n{year_start: N+1}"]

    SS -->|"Broker.generate_submissions()\n→ spread over first 30 days"| SA
    SS -->|"perils::schedule_loss_events\nPoisson(λ) per PerilConfig\n(cat + attritional)"| LE
    SS -->|"schedule day 365"| YE

    %% ── Quoting round ───────────────────────────────────────────────────────

    subgraph Broker
        SA["**SubmissionArrived**\n{submission_id, broker_id, risk}"]
    end

    subgraph Market["Market (Coordinator)"]
        QR_L["**QuoteRequested**\n{is_lead: true}"]
        QR_F["**QuoteRequested**\n{is_lead: false}\n+1 day"]
        PB["**PolicyBound**\n{submission_id, panel}\n+2 days"]
        LE_D["on_loss_event\nmatches territory + peril\napplies attachment/limit"]
        STATS["compute_year_stats\n→ industry_loss_ratio\n→ YTD reset\n→ expire year policies"]
    end

    subgraph Syndicate["Syndicate  (ATP pricing)"]
        QI_L["**QuoteIssued**\n{is_lead: true, premium}\nATP blended with benchmark"]
        QD_L["**QuoteDeclined**\n{submission_id, syndicate_id}"]
        QI_F["**QuoteIssued**\n{is_lead: false, premium}\nfollower pricing vs lead"]
        QD_F["**QuoteDeclined**\n{submission_id, syndicate_id}"]
        CS_S["on_claim_settled\ncapital −= amount"]
        YE_S["on_year_end\nEWMA ← realised loss ratio\n(per line of business)"]
    end

    SA --> QR_L
    QR_L --> QI_L
    QR_L --> QD_L
    QI_L -->|"record lead premium\ninvite all other syndicates"| QR_F
    QD_L -->|"no followers if lead declines\n(submission abandoned)"| ABANDON(["submission abandoned\n(silent)"])
    QR_F --> QI_F
    QR_F --> QD_F
    QI_F -->|"all followers responded"| PB
    QD_F -->|"all followers responded"| PB

    %% ── Loss cascade ────────────────────────────────────────────────────────

    CS["**ClaimSettled**\n{policy_id, syndicate_id, amount}"]

    LE --> LE_D
    LE_D -->|"one ClaimSettled\nper panel entry"| CS
    CS --> CS_S
    CS -->|"accumulate ytd_claims_by_line"| STATS

    PB -->|"accumulate ytd_premiums_by_line"| STATS

    %% ── Year end ────────────────────────────────────────────────────────────

    YE --> STATS
    STATS -->|"publish current_industry_benchmark\nfor next year's ATP"| YE_S
    YE -->|"schedule SimulationStart(N+1)"| SS_NEXT

    %% ── Entry / insolvency (stubs) ──────────────────────────────────────────

    SE(["**SyndicateEntered**\n{syndicate_id}\n(handler: no-op)"])
    SI(["**SyndicateInsolvency**\n{syndicate_id}\n(handler: no-op)"])
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
| 3 | `SubmissionArrived` | `Broker::generate_submissions` | `Market::on_submission_arrived` |
| 4 | `QuoteRequested` | `Market::on_submission_arrived`, `Market::on_lead_quote_issued` | `Syndicate::on_quote_requested` |
| 5 | `QuoteIssued` | `Syndicate::on_quote_requested` | `Market::on_lead_quote_issued` / `Market::on_follower_quote_issued` |
| 6 | `QuoteDeclined` | `Syndicate::on_quote_requested` | `Market::on_quote_declined` |
| 7 | `PolicyBound` | `Market::assemble_panel` (+2 days) | `Market::on_policy_bound` (registers policy, YTD premium) |
| 8 | `LossEvent` | `handle_simulation_start` via `perils::schedule_loss_events` (Poisson frequency-severity) | `Market::on_loss_event` |
| 9 | `ClaimSettled` | `Market::on_loss_event` | `Syndicate::on_claim_settled` + `Market::on_claim_settled` (YTD) |
| 10 | `SyndicateEntered` | — | no-op |
| 11 | `SyndicateInsolvency` | — | no-op |

## Day offsets

- `SubmissionArrived` → `QuoteRequested` (lead): **same day**
- Lead `QuoteIssued` → `QuoteRequested` (followers): **+1 day**
- Last follower response → `PolicyBound`: **+2 days**
- `LossEvent` → `ClaimSettled`: **same day**
- `SimulationStart` → `YearEnd`: **day 365 of that year**
- `YearEnd` → next `SimulationStart`: **day 1 of next year**
