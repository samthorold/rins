#!/usr/bin/env python3
"""
verify_claims.py — event-stream verifier for rins claim correctness.

With the grounded loss model, the full chain is:
  LossEvent → InsuredLoss {ground_up_loss} → ClaimSettled

Two checks are performed:
  1. InsuredLoss.ground_up_loss ≤ sum_insured for every InsuredLoss event.
  2. For each InsuredLoss, the ClaimSettled amounts on the same day for the same
     policy_id satisfy: sum(amounts) == min(ground_up_loss, limit) − attachment
     (with minor integer-rounding tolerance).

Both cat and attritional losses now flow through InsuredLoss, so a single pass
covers both.

Run from the project root after `cargo run`:
    python3 scripts/verify_claims.py
"""
import json, sys
from collections import defaultdict
from pathlib import Path

events_path = Path("events.ndjson")
events = [json.loads(l) for l in events_path.read_text().splitlines() if l.strip()]

# ── Build policy metadata from SubmissionArrived + PolicyBound ───────────────

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
            "limit": risk["limit"],
            "attachment": risk["attachment"],
            "sum_insured": risk["sum_insured"],
            "territory": risk["territory"],
            "perils": risk["perils_covered"],
            "entries": v["panel"]["entries"],
            "bound_day": e["day"],
        }
        policy_counter += 1

print(f"Policies loaded: {len(policies)}")

# ── Index InsuredLoss and ClaimSettled events ─────────────────────────────────

# (day, policy_id) → [ground_up_loss, ...] (from InsuredLoss; list because multiple events
# can share the same (day, policy) — e.g. same-day attritional + cat collision)
insured_loss_index = defaultdict(list)
# (day, policy_id) → {syndicate_id: total_amount} (from ClaimSettled)
claim_index = defaultdict(lambda: defaultdict(int))

insured_loss_count = 0
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k == "InsuredLoss":
        insured_loss_index[(day, v["policy_id"])].append(v["ground_up_loss"])
        insured_loss_count += 1
    elif k == "ClaimSettled":
        claim_index[(day, v["policy_id"])][v["syndicate_id"]] += v["amount"]

print(f"InsuredLoss events: {insured_loss_count}")

mismatches = []
ground_up_checks = 0
claim_checks = 0

# ── Check 1: ground_up_loss ≤ sum_insured (per individual InsuredLoss event) ───

for (day, pid), losses in insured_loss_index.items():
    policy = policies.get(pid)
    if policy is None:
        mismatches.append(f"  FAIL day={day}: InsuredLoss references unknown policy_id {pid}")
        continue
    for ground_up_loss in losses:
        if ground_up_loss > policy["sum_insured"]:
            mismatches.append(
                f"  FAIL day={day} policy={pid}: ground_up_loss={ground_up_loss} "
                f"> sum_insured={policy['sum_insured']}"
            )
        ground_up_checks += 1

print(f"Ground-up checks (ground_up ≤ sum_insured): {ground_up_checks}")

# ── Check 2: ClaimSettled amounts match policy terms applied to ground_up_loss ─
# Each InsuredLoss event independently produces ClaimSettled events via on_insured_loss,
# so expected total = sum of net amounts across all same-day InsuredLoss events.

for (day, pid), losses in insured_loss_index.items():
    policy = policies.get(pid)
    if policy is None:
        continue  # already reported above

    # Expected per-syndicate amounts: mirror Rust exactly — each InsuredLoss event
    # independently calls on_insured_loss, which applies net * share_bps / 10_000
    # (integer division) per panel entry. Sum those independently to avoid ±1 drift.
    expected_by_syn = defaultdict(int)
    total_net = 0
    for ground_up_loss in losses:
        gross = min(ground_up_loss, policy["limit"])
        net = max(0, gross - policy["attachment"])
        total_net += net
        for entry in policy["entries"]:
            amount = net * entry["share_bps"] // 10_000
            if amount > 0:
                expected_by_syn[entry["syndicate_id"]] += amount

    if total_net == 0:
        # No claim expected; ensure no ClaimSettled exists for this (day, pid).
        actual = claim_index.get((day, pid), {})
        if actual:
            mismatches.append(
                f"  FAIL day={day} policy={pid}: total_net=0 but ClaimSettled found: {dict(actual)}"
            )
        continue

    actual_by_syn = claim_index.get((day, pid), {})

    for syn_id in sorted(set(expected_by_syn) | set(actual_by_syn)):
        expected = expected_by_syn.get(syn_id, 0)
        actual = actual_by_syn.get(syn_id, 0)
        if expected != actual:
            mismatches.append(
                f"  FAIL day={day} policy={pid} syn={syn_id}: "
                f"expected {expected} but got {actual} "
                f"(losses={losses}, limit={policy['limit']}, "
                f"attachment={policy['attachment']}, total_net={total_net})"
            )
        claim_checks += 1

# ── Check 3: No ClaimSettled without a matching InsuredLoss ───────────────────

for (day, pid) in claim_index:
    if not insured_loss_index[(day, pid)]:
        mismatches.append(
            f"  FAIL day={day} policy={pid}: ClaimSettled with no matching InsuredLoss"
        )

print(f"Claim amount checks (ClaimSettled vs policy terms): {claim_checks}")

# ── Result ────────────────────────────────────────────────────────────────────

if mismatches:
    print(f"\nFAIL — {len(mismatches)} mismatch(es):")
    for m in mismatches[:50]: print(m)
    if len(mismatches) > 50: print(f"  ... and {len(mismatches) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — all InsuredLoss and ClaimSettled amounts are consistent.")
