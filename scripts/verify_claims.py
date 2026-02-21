#!/usr/bin/env python3
"""
verify_claims.py — event-stream verifier for rins claim-splitting correctness.

Two classes of ClaimSettled are handled separately:
  - Cat claims: produced by on_loss_event; verified against the parent LossEvent.
  - Attritional claims: produced by schedule_attritional_claims_for_policy at
    bind time (no LossEvent parent); verified against policy limit/attachment bounds.

A ClaimSettled is classified as attritional when the policy covers
Peril::Attritional and no LossEvent with a matching territory+cat-peril exists
on the same day.

Run from the project root after `cargo run`:
    python3 scripts/verify_claims.py
"""
import json, sys
from collections import defaultdict
from pathlib import Path

events_path = Path("events.ndjson")
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
# claim_index keyed by (day, policy_id) → {syndicate_id: total_amount}
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

# ── Cat claim verification ────────────────────────────────────────────────────
# For each loss day × policy, verify ClaimSettled amounts match expected values.
# Attritional-only policies are skipped here (they have no LossEvent parent).

cat_claim_days_policies = set()  # (day, pid) pairs verified as cat claims

for day in sorted(loss_index.keys()):
    losses = loss_index[day]
    for pid, policy in policies.items():
        # Only check policies strictly bound before this loss day and in the same
        # policy year — Lloyd's policies are annual and expire at YearEnd.
        if policy["bound_day"] >= day:
            continue
        if policy["bound_day"] // 360 != day // 360:
            continue

        # Compute expected cat claims (excluding Attritional peril).
        expected_by_syn = defaultdict(int)
        total_expected_net = 0
        for loss in losses:
            if loss["peril"] == "Attritional": continue  # never routed via LossEvent now
            if loss["peril"] not in policy["perils"]: continue
            if loss["region"] != policy["territory"]: continue
            net_loss = max(0, min(loss["severity"], policy["limit"]) - policy["attachment"])
            if net_loss == 0: continue
            total_expected_net += net_loss
            for entry in policy["entries"]:
                expected_by_syn[entry["syndicate_id"]] += net_loss * entry["share_bps"] // 10_000

        if not expected_by_syn:
            # No cat match — any claims on this day must be attritional.
            # Skip; attritional claims for this (day, pid) are checked below.
            continue

        cat_claim_days_policies.add((day, pid))
        actual_by_syn = claim_index.get((day, pid), {})
        total_actual = sum(actual_by_syn.values())
        if total_actual > total_expected_net:
            mismatches.append(
                f"  FAIL day={day} policy={pid}: total_actual={total_actual} > total_expected_net={total_expected_net}"
            )
        for syn_id in sorted(set(expected_by_syn) | set(actual_by_syn)):
            expected = expected_by_syn.get(syn_id, 0)
            actual = actual_by_syn.get(syn_id, 0)
            if expected != actual:
                mismatches.append(
                    f"  FAIL day={day} policy={pid} syn={syn_id}: expected {expected} but got {actual}"
                )
            checks_run += 1

# ── Attritional claim bounds verification ────────────────────────────────────
# For each (day, policy_id) in claim_index that was NOT verified as a cat claim,
# check that every amount is ≤ (limit − attachment) × share_bps / 10_000.

attritional_checks = 0
for (day, pid), by_syn in claim_index.items():
    if (day, pid) in cat_claim_days_policies:
        continue  # already verified as cat
    policy = policies.get(pid)
    if policy is None:
        mismatches.append(f"  FAIL day={day}: ClaimSettled references unknown policy_id {pid}")
        continue

    # Build max-amount lookup from panel entries.
    max_net = policy["limit"] - policy["attachment"]
    share_by_syn = {entry["syndicate_id"]: entry["share_bps"] for entry in policy["entries"]}

    for syn_id, amount in by_syn.items():
        max_allowed = max_net * share_by_syn.get(syn_id, 0) // 10_000
        if amount > max_allowed:
            mismatches.append(
                f"  FAIL day={day} policy={pid} syn={syn_id}: attritional amount {amount} "
                f"> max_allowed {max_allowed} (limit−attachment={max_net})"
            )
        attritional_checks += 1

print(f"Cat claim checks: {checks_run}")
print(f"Attritional claim bounds checks: {attritional_checks}")

if mismatches:
    print(f"\nFAIL — {len(mismatches)} mismatch(es):")
    for m in mismatches[:50]: print(m)
    if len(mismatches) > 50: print(f"  ... and {len(mismatches) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — all claim amounts match expected values.")
