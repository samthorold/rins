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

**Insured and GUL diagnostics:**
- Note total GUL vs total claims settled each year — a large gap indicates losses absorbed by insureds above attachment or below deductible
- Comment on insured GUL concentration (GUL-HHI and top-insured share): a few dominant insureds suggest limited diversification of the insured book
- Flag if a single insured's GUL spikes sharply in a cat year — sign that a specific insured is driving systemic risk
- Note the count of distinct insureds generating losses each year; a shrinking count in later years may indicate capacity withdrawal or portfolio narrowing
- Compare year-over-year GUL per insured to identify whether loss growth is broad-based or concentrated in repeat high-loss names

- Suggest one follow-up question or area to investigate based on the data
