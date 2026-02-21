#!/usr/bin/env python3
"""
verify_claims.py — event-stream verifier for rins claim correctness.

With the grounded loss model + decoupled GUL, the full chain is:
  LossEvent → InsuredLoss {ground_up_loss, policy_id: Option} → ClaimSettled?

InsuredLoss.policy_id may be:
  - Some(pid) : insured has a bound policy; ClaimSettled events follow if net > 0
  - None       : uninsured or attritional without pre-known policy; on_insured_loss
                 resolves via insured_active_policies at fire time — may produce
                 ClaimSettled with the resolved effective policy_id

Three checks are performed:
  1. InsuredLoss.ground_up_loss ≤ sum_insured for every InsuredLoss event.
     For null policy_id, sum_insured is bounded by max across all SubmissionArrived
     for that insured.
  2. For each InsuredLoss with a non-null policy_id, the ClaimSettled amounts
     on the same day satisfy: sum(amounts) == min(ground_up_loss, limit) − attachment
     (with minor integer-rounding tolerance). InsuredLoss(None) events: skip.
  3. Every ClaimSettled must have a matching InsuredLoss on the same day. A match
     is either:
       (a) InsuredLoss { policy_id: Some(pid) } for the same pid, OR
       (b) InsuredLoss { policy_id: None } for the insured of that policy
           (resolved path via insured_active_policies).

Run from the project root after `cargo run`:
    python3 scripts/verify_claims.py
"""
import json, sys
from collections import defaultdict
from pathlib import Path

events_path = Path("events.ndjson")
events = [json.loads(l) for l in events_path.read_text().splitlines() if l.strip()]

# ── Build metadata from SubmissionArrived + PolicyBound ──────────────────────

submission_risk = {}
submission_insured = {}  # submission_id → insured_id
insured_max_sum_insured = defaultdict(int)  # insured_id → max sum_insured seen

for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "SubmissionArrived":
        submission_risk[v["submission_id"]] = v["risk"]
        submission_insured[v["submission_id"]] = v["insured_id"]
        si = v["risk"]["sum_insured"]
        iid = v["insured_id"]
        if si > insured_max_sum_insured[iid]:
            insured_max_sum_insured[iid] = si

policies = {}       # policy_counter → policy dict
policy_insured = {} # policy_counter → insured_id
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
        iid = submission_insured.get(sid)
        if iid is not None:
            policy_insured[policy_counter] = iid
        policy_counter += 1

print(f"Policies loaded: {len(policies)}")

# ── Index InsuredLoss and ClaimSettled events ─────────────────────────────────

# Key: (day, policy_id_or_"null", insured_id) → [ground_up_loss, ...]
insured_loss_index = defaultdict(list)
# (day, policy_id) → {syndicate_id: total_amount} for non-null policy_id only
claim_index = defaultdict(lambda: defaultdict(int))
# Set of (day, policy_id) pairs from InsuredLoss with non-null policy_id
insured_loss_with_pid = set()
# Set of (day, insured_id) pairs from InsuredLoss(None)
null_loss_by_day_insured = set()

insured_loss_count = 0
null_policy_count = 0
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k == "InsuredLoss":
        pid = v["policy_id"]
        iid = v["insured_id"]
        if pid is None:
            insured_loss_index[(day, "null", iid)].append(v["ground_up_loss"])
            null_loss_by_day_insured.add((day, iid))
            null_policy_count += 1
        else:
            insured_loss_index[(day, pid, iid)].append(v["ground_up_loss"])
            insured_loss_with_pid.add((day, pid))
        insured_loss_count += 1
    elif k == "ClaimSettled":
        claim_index[(day, v["policy_id"])][v["syndicate_id"]] += v["amount"]

print(f"InsuredLoss events: {insured_loss_count} ({null_policy_count} with policy_id=null)")

mismatches = []
ground_up_checks = 0
claim_checks = 0

# ── Check 1: ground_up_loss ≤ sum_insured ────────────────────────────────────

for (day, pid, iid), losses in insured_loss_index.items():
    if pid == "null":
        max_si = insured_max_sum_insured.get(iid)
        if max_si is None:
            mismatches.append(
                f"  FAIL day={day}: InsuredLoss(None) for insured_id={iid} "
                f"has no SubmissionArrived to bound sum_insured"
            )
            continue
        for ground_up_loss in losses:
            if ground_up_loss > max_si:
                mismatches.append(
                    f"  FAIL day={day} insured={iid}: uninsured ground_up_loss={ground_up_loss} "
                    f"> max_sum_insured={max_si}"
                )
            ground_up_checks += 1
    else:
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

# ── Check 2: ClaimSettled amounts match policy terms ─────────────────────────
# Only for InsuredLoss events with a non-null policy_id.

for (day, pid, iid), losses in insured_loss_index.items():
    if pid == "null":
        continue  # Uninsured: ClaimSettled may or may not follow; verify via Check 3.

    policy = policies.get(pid)
    if policy is None:
        continue  # already reported above

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
# A ClaimSettled(day, pid) is valid if:
#   (a) There is an explicit InsuredLoss(Some(pid)) on the same day, OR
#   (b) The insured of policy pid has an InsuredLoss(None) on the same day
#       (the None event resolved to this policy via insured_active_policies).

for (day, pid) in claim_index:
    if (day, pid) in insured_loss_with_pid:
        continue  # (a) direct explicit match

    # (b) check if the insured of this policy had an InsuredLoss(None) on the same day
    insured_for_policy = policy_insured.get(pid)
    if insured_for_policy is not None and (day, insured_for_policy) in null_loss_by_day_insured:
        continue  # (b) resolved via None path

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
