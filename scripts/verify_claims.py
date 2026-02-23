#!/usr/bin/env python3
"""
verify_claims.py — event-stream verifier for rins claim correctness.

Full coverage model (limit = sum_insured, attachment = 0):
  InsuredLoss → ClaimSettled (amount capped by remaining asset value per policy per year)

Three checks:
  1. InsuredLoss.ground_up_loss ≤ sum_insured for the bound policy.
  2. Aggregate ClaimSettled per (policy_id, year) ≤ sum_insured.
  3. Every ClaimSettled has a matching InsuredLoss on the same day with the same policy_id.

Run from the project root after `cargo run`:
    python3 scripts/verify_claims.py
"""
import sys
from collections import defaultdict
import os; sys.path.insert(0, os.path.dirname(__file__))
from event_index import build_index, year

idx = build_index()

# ── Build metadata ────────────────────────────────────────────────────────────

# policy_id -> {sum_insured, insurer_id, submission_id, bound_day}
policies = {}
for pid, iid in idx.policy_insurer.items():
    sid = idx.policy_sub.get(pid)
    si = idx.policy_sum_insured.get(pid)
    if si is None:
        print(f"WARN: PolicyBound for submission {sid} has no LeadQuoteRequested")
        continue
    policies[pid] = {
        "sum_insured": si,
        "insurer_id":  iid,
        "submission_id": sid,
        "bound_day": idx.policy_bound_day.get(pid),
    }

print(f"Policies loaded: {len(policies)}")

# ── Index InsuredLoss and ClaimSettled ────────────────────────────────────────

# (day, policy_id) -> list of ground_up_loss values
insured_loss_index = defaultdict(list)
for d in idx.insured_losses:
    insured_loss_index[(d["day"], d["policy_id"])].append(d["ground_up_loss"])

# (policy_id, year) -> total ClaimSettled amount
claim_totals = defaultdict(int)
# (day, policy_id) seen in ClaimSettled
claim_day_pids = set()

for d in idx.claim_settled:
    pid = d["policy_id"]
    claim_totals[(pid, year(d["day"]))] += d["amount"]
    claim_day_pids.add((d["day"], pid))

insured_loss_count = len(idx.insured_losses)
claim_count = len(idx.claim_settled)

print(f"InsuredLoss events: {insured_loss_count}")
print(f"ClaimSettled events: {claim_count}")

mismatches = []
ground_up_checks = 0
aggregate_checks = 0
orphan_checks = 0

# ── Check 1: ground_up_loss ≤ sum_insured ────────────────────────────────────

for (day, pid), losses in insured_loss_index.items():
    policy = policies.get(pid)
    if policy is None:
        mismatches.append(f"  FAIL day={day}: InsuredLoss references unknown policy_id={pid}")
        continue
    si = policy["sum_insured"]
    for gul in losses:
        if gul > si:
            mismatches.append(
                f"  FAIL day={day} policy={pid}: ground_up_loss={gul} > sum_insured={si}"
            )
        ground_up_checks += 1

print(f"Ground-up checks (ground_up_loss ≤ sum_insured): {ground_up_checks}")

# ── Check 2: aggregate ClaimSettled per (policy, year) ≤ sum_insured ─────────

for (pid, y), total_claimed in claim_totals.items():
    policy = policies.get(pid)
    if policy is None:
        mismatches.append(f"  FAIL: ClaimSettled for unknown policy_id={pid} in year {y}")
        continue
    si = policy["sum_insured"]
    if total_claimed > si:
        mismatches.append(
            f"  FAIL policy={pid} year={y}: aggregate ClaimSettled={total_claimed} > sum_insured={si}"
        )
    aggregate_checks += 1

print(f"Aggregate cap checks (sum ClaimSettled per policy/year ≤ sum_insured): {aggregate_checks}")

# ── Check 3: every ClaimSettled has a matching InsuredLoss ───────────────────

for (day, pid) in claim_day_pids:
    if (day, pid) not in insured_loss_index:
        mismatches.append(
            f"  FAIL day={day} policy={pid}: ClaimSettled with no matching InsuredLoss on same day"
        )
    orphan_checks += 1

print(f"Orphan ClaimSettled checks: {orphan_checks}")

# ── Result ────────────────────────────────────────────────────────────────────

if mismatches:
    print(f"\nFAIL — {len(mismatches)} mismatch(es):")
    for m in mismatches[:50]: print(m)
    if len(mismatches) > 50: print(f"  ... and {len(mismatches) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — all InsuredLoss and ClaimSettled amounts are consistent.")
