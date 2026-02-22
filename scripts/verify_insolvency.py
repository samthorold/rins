#!/usr/bin/env python3
"""
verify_insolvency.py — event-stream verifier for insurer capital integrity.

Replaces the old SyndicateInsolvency uniqueness check (no insolvency events
in this model; capital can go negative by design).

Invariants:
  1. Every ClaimSettled.amount is positive (> 0).
  2. Every ClaimSettled references a policy_id that was previously bound.
  3. Every ClaimSettled references an insurer_id consistent with the PolicyBound
     for that policy_id.

Run from the project root after `cargo run`:
    python3 scripts/verify_insolvency.py
"""
import json, sys
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

# Build policy_id -> insurer_id from PolicyBound
policy_insurer = {}
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "PolicyBound":
        policy_insurer[v["policy_id"]] = v["insurer_id"]

failures = []
claim_count = 0

for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k != "ClaimSettled": continue
    claim_count += 1
    pid = v["policy_id"]
    iid = v["insurer_id"]
    amt = v["amount"]

    # Check 1: positive amount
    if amt <= 0:
        failures.append(f"  FAIL day={day} policy={pid}: ClaimSettled amount={amt} is not positive")

    # Check 2 & 3: policy known and insurer consistent
    expected_iid = policy_insurer.get(pid)
    if expected_iid is None:
        failures.append(f"  FAIL day={day}: ClaimSettled references unknown policy_id={pid}")
    elif expected_iid != iid:
        failures.append(
            f"  FAIL day={day} policy={pid}: ClaimSettled insurer={iid} "
            f"but PolicyBound insurer={expected_iid}"
        )

print(f"ClaimSettled events checked: {claim_count}")
print(f"Policies tracked: {len(policy_insurer)}")

if failures:
    print(f"\nFAIL — {len(failures)} violation(s):")
    for f in failures[:50]:
        print(f)
    if len(failures) > 50:
        print(f"  ... and {len(failures) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — all ClaimSettled amounts are positive and reference valid policies/insurers.")
