---
name: analyse-sim
description: Regenerate the rins simulation and produce a structured year-over-year analysis of events.ndjson
user-invocable: true
allowed-tools: Bash
---

Follow these steps exactly. Do not skip regeneration.

## Step 1 — Regenerate

Run `cargo run` in /Users/sam/Projects/rins to produce a fresh events.ndjson.
Report any build or runtime errors and stop if the run fails.

## Step 2 — Analyse and verify

From /Users/sam/Projects/rins, run:

```
python3 scripts/analyse_sim.py
python3 scripts/verify_claims.py
python3 scripts/verify_insolvency.py
python3 scripts/verify_panel_integrity.py
python3 scripts/verify_quoting_flow.py
```

Report any FAIL lines from each verifier before the Step 3 analysis.

## Step 3 — Report

Present the analysis output with brief interpretation:
- Highlight any year-over-year trends (rising/falling bind rates, premium shifts, loss spikes)
- Note any event types with zero counts that should be non-zero (potential bugs)
- Flag if total claims significantly exceed total premiums for any syndicate in any year
- Comment on capacity trends: are insolvency counts rising, and is the active syndicate count shrinking over time?
- Comment on HHI trend: does market concentration surge after catastrophe years?
- Note if average panel size is shrinking (sign of capacity withdrawal)
- Flag if lead role is highly concentrated (top 1-2 syndicates holding >50% of leads)
- Suggest one follow-up question or area to investigate based on the data
