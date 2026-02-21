---
name: verify-events
description: Verify claim-splitting correctness in events.ndjson by checking ClaimSettled amounts against expected values derived from PolicyBound and LossEvent data
user-invocable: true
allowed-tools: Bash
---

Verify structural invariants in `events.ndjson`.

## Step 1 — Regenerate

Run `cargo run` in /Users/sam/Projects/rins to produce a fresh `events.ndjson`.
Report any build or runtime errors and stop if the run fails.

## Step 2 — Run all verifiers

From /Users/sam/Projects/rins, run:

```
python3 scripts/verify_claims.py
python3 scripts/verify_insolvency.py
python3 scripts/verify_panel_integrity.py
python3 scripts/verify_quoting_flow.py
```

## Step 3 — Report

Report PASS or FAIL for each verifier. On failure, list FAIL lines from each script before the analysis. For `verify_claims.py` failures, include the policy, day, syndicate, expected value, and actual value.
