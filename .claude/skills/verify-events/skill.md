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

## Step 2 — Write and run the verifier

Write the following Python script to `/tmp/verify_claims.py` and run it with `python3 /tmp/verify_claims.py --no-regen` from /Users/sam/Projects/rins:

```python
#!/usr/bin/env python3
"""
verify_claims.py — event-stream verifier for rins claim-splitting correctness.
Groups by (day, policy) and sums across ALL matching LossEvents on that day
before comparing to actual ClaimSettled totals per syndicate.
"""
import json, sys
from collections import defaultdict
from pathlib import Path

events_path = Path("/Users/sam/Projects/rins/events.ndjson")
events = [json.loads(l) for l in events_path.read_text().splitlines() if l.strip()]

submission_risk = {}
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "SubmissionArrived":
        submission_risk[v["submission_id"]] = v["risk"]

policies = {}
policy_counter = 0
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "PolicyBound":
        sid = v["submission_id"]
        risk = submission_risk.get(sid)
        if risk is None:
            print(f"WARN: PolicyBound for submission {sid} has no SubmissionArrived")
            policy_counter += 1; continue
        policies[policy_counter] = {
            "limit": risk["limit"], "attachment": risk["attachment"],
            "territory": risk["territory"], "perils": risk["perils_covered"],
            "entries": v["panel"]["entries"], "bound_day": e["day"],
        }
        policy_counter += 1

print(f"Policies loaded: {len(policies)}")

loss_index = defaultdict(list)
claim_index = defaultdict(lambda: defaultdict(int))
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k == "LossEvent":
        loss_index[day].append({"region": v["region"], "peril": v["peril"], "severity": v["severity"]})
    elif k == "ClaimSettled":
        claim_index[(day, v["policy_id"])][v["syndicate_id"]] += v["amount"]

mismatches = []
checks_run = 0

for day in sorted(loss_index.keys()):
    losses = loss_index[day]
    for pid, policy in policies.items():
        # Only check policies strictly bound before this loss day.
        if policy["bound_day"] >= day:
            continue
        expected_by_syn = defaultdict(int)
        total_expected_net = 0
        for loss in losses:
            if loss["peril"] not in policy["perils"]: continue
            if loss["region"] != policy["territory"]: continue
            net_loss = max(0, min(loss["severity"], policy["limit"]) - policy["attachment"])
            if net_loss == 0: continue
            total_expected_net += net_loss
            for entry in policy["entries"]:
                expected_by_syn[entry["syndicate_id"]] += net_loss * entry["share_bps"] // 10_000
        if not expected_by_syn:
            actual = claim_index.get((day, pid), {})
            if actual:
                mismatches.append(f"  FAIL day={day} policy={pid}: no matching loss but got claims {dict(actual)}")
                checks_run += 1
            continue
        actual_by_syn = claim_index.get((day, pid), {})
        total_actual = sum(actual_by_syn.values())
        if total_actual > total_expected_net:
            mismatches.append(f"  FAIL day={day} policy={pid}: total_actual={total_actual} > total_expected_net={total_expected_net}")
        for syn_id in sorted(set(expected_by_syn) | set(actual_by_syn)):
            expected = expected_by_syn.get(syn_id, 0)
            actual = actual_by_syn.get(syn_id, 0)
            if expected != actual:
                mismatches.append(f"  FAIL day={day} policy={pid} syn={syn_id}: expected {expected} but got {actual}")
            checks_run += 1

print(f"Checks run: {checks_run}")
if mismatches:
    print(f"\nFAIL — {len(mismatches)} mismatch(es):")
    for m in mismatches[:50]: print(m)
    if len(mismatches) > 50: print(f"  ... and {len(mismatches) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — all claim amounts match expected values.")
```

Run: `python3 /tmp/verify_claims.py --no-regen`

## Step 3 — Report

Report PASS or FAIL. On failure, list each mismatch with the policy, day, syndicate, expected value, and actual value.
