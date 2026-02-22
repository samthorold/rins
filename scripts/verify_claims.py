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
import json, sys
from collections import defaultdict
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

def year(day): return day // 360 + 1

# ── Build metadata ────────────────────────────────────────────────────────────

# submission_id -> sum_insured (from LeadQuoteRequested which carries the risk)
submission_sum_insured = {}
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "LeadQuoteRequested":
        submission_sum_insured[v["submission_id"]] = v["risk"]["sum_insured"]

# policy_id -> {sum_insured, insurer_id, submission_id} (from PolicyBound)
policies = {}
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "PolicyBound":
        sid = v["submission_id"]
        si = submission_sum_insured.get(sid)
        if si is None:
            print(f"WARN: PolicyBound for submission {sid} has no LeadQuoteRequested")
            continue
        policies[v["policy_id"]] = {
            "sum_insured": si,
            "insurer_id":  v["insurer_id"],
            "submission_id": sid,
            "bound_day": e["day"],
        }

print(f"Policies loaded: {len(policies)}")

# ── Index InsuredLoss and ClaimSettled ────────────────────────────────────────

# (day, policy_id) -> list of ground_up_loss values
insured_loss_index = defaultdict(list)

# (policy_id, year) -> total ClaimSettled amount
claim_totals = defaultdict(int)
# (day, policy_id) seen in ClaimSettled
claim_day_pids = set()

insured_loss_count = 0
claim_count = 0

for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k == "InsuredLoss":
        pid = v["policy_id"]
        insured_loss_index[(day, pid)].append(v["ground_up_loss"])
        insured_loss_count += 1
    elif k == "ClaimSettled":
        pid = v["policy_id"]
        y   = year(day)
        claim_totals[(pid, y)] += v["amount"]
        claim_day_pids.add((day, pid))
        claim_count += 1

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
