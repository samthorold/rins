---
name: verify-events
description: Verify claim-splitting correctness in events.ndjson by checking ClaimSettled amounts against expected values derived from PolicyBound and LossEvent data
user-invocable: true
allowed-tools: Bash
---

Verify that all `ClaimSettled` amounts in `events.ndjson` are arithmetically correct.

## Step 1 — Regenerate

Run `cargo run` in /Users/sam/Projects/rins to produce a fresh `events.ndjson`.
Report any build or runtime errors and stop if the run fails.

## Step 2 — Run the verifier

From /Users/sam/Projects/rins, run:

```
python3 scripts/verify_claims.py
```

## Step 3 — Report

Report PASS or FAIL. On failure, list each mismatch with the policy, day, syndicate, expected value, and actual value.
