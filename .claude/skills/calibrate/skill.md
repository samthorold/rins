---
name: calibrate
description: Regenerate the rins simulation and compare output against Lloyd's market benchmarks, identifying the top calibration gaps and suggesting targeted parameter fixes.
user-invocable: true
allowed-tools: Bash
---

Run through all steps in order. Do not skip regeneration.

## Step 1 — Regenerate

Run `cargo run` in /Users/sam/Projects/rins to produce a fresh `events.ndjson`.
Report build or runtime errors and stop if the run fails.

## Step 2 — Run calibration check

From /Users/sam/Projects/rins, run:

```
python3 scripts/calibrate.py
```

## Step 3 — Report and interpret

Present the full output of the calibration script, then provide brief interpretation:

### Read the delta table
- Report how many metrics are FAIL / WARN / PASS.
- Note any metric that has regressed since the last session (if there is memory context).

### Assess the top 3 suggestions
- For each suggestion, state whether it is an architecture issue (needs code change) or a parameter issue (needs config tuning).
- Identify the single highest-leverage fix: the one change most likely to move the most FAIL metrics toward PASS simultaneously.

### Benchmark context
The benchmarks reflect Lloyd's market data 2018–2023:
- **Bind rate 50–80%**: specialty market places most risks presented
- **Insolvency <5%/yr**: Lloyd's virtually never sees mass insolvency; even Katrina 2005 did not produce >5% syndicate failure
- **Combined ratio 70–110%**: market-level; individual syndicates can run 150%+ in a single bad year
- **Lead HHI 200–800**: with 80 syndicates, leadership should be distributed; top lead syndicates hold ~15–25% share
- **Panel 5–15**: typical Lloyd's risk placed with 8–15 syndicates
- **Survival >90%/yr**: capacity should rebuild between cat events, not accumulate insolvencies year-on-year

### What NOT to do
- Do not attempt to fix all FAIL metrics in one pass.
- Do not tune parameters that would hardcode emergent phenomena (e.g. do not force a specific combined ratio target).
- The goal is incremental movement toward correctness — propose one focused change per session.

### Suggest next action
End with a single concrete recommendation: one parameter change or one code behaviour to investigate, with the specific file and location where the change should be made.
