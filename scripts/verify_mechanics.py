#!/usr/bin/env python3
"""
verify_mechanics.py — event-stream verifier for rins mechanics invariants.

MECHANICS INVARIANTS — derived from docs/market-mechanics.md [ACTIVE] sections.
Review and update this script whenever docs/market-mechanics.md is updated.

Six invariant checks:
  [1] Day-offset chain: LeadQuoteIssued same day as Requested; QuotePresented +1;
      QuoteAccepted same day; PolicyBound +1.
  [2] Loss eligibility: no InsuredLoss before PolicyBound day.
  [3] Attritional InsuredLoss strictly after PolicyBound (day > bound_day).
  [4] PolicyExpired timing: PolicyExpired.day == PolicyBound.day + 360.
  [5] No post-expiry claims: ClaimSettled.day < PolicyExpired.day.
  [6] Cat damage fraction consistency: shared draw per (WindstormAtlantic, day).

Run from the project root after `cargo run --release`:
    python3 scripts/verify_mechanics.py
"""
import sys
from collections import defaultdict
import os; sys.path.insert(0, os.path.dirname(__file__))
from event_index import build_index

idx = build_index()

# ── Unpack index fields ────────────────────────────────────────────────────────

sub_req_day       = idx.sub_request_day
sub_issued_day    = idx.sub_issued_day
sub_presented_day = idx.sub_presented_day
sub_accepted_day  = idx.sub_accepted_day
policy_bound_day  = idx.policy_bound_day
policy_expire_day = idx.policy_expiry_day
policy_sum_insured = idx.policy_sum_insured
max_day           = idx.max_day

# sub_policy_bound: submission_id -> (policy_id, day)
sub_policy_bound = {
    sid: (pid, idx.policy_bound_day[pid])
    for sid, pid in idx.sub_policy.items()
    if pid in idx.policy_bound_day
}

# insured_loss_list: [(day, policy_id, peril, gul)]
insured_loss_list = [
    (d["day"], d["policy_id"], d["peril"], d["ground_up_loss"])
    for d in idx.insured_losses
]

# insured_losses_by_peril_day: (peril, day) -> [(policy_id, gul)]
insured_losses_by_peril_day = defaultdict(list)
for d in idx.insured_losses:
    insured_losses_by_peril_day[(d["peril"], d["day"])].append((d["policy_id"], d["ground_up_loss"]))

# claim_settled_list: [(day, policy_id)]
claim_settled_list = [(d["day"], d["policy_id"]) for d in idx.claim_settled]

# loss_events_per_peril_day: (peril, day) -> count
loss_events_per_peril_day = defaultdict(int)
for d in idx.loss_events:
    loss_events_per_peril_day[(d["peril"], d["day"])] += 1

# ── Collect violations ────────────────────────────────────────────────────────

v1_fail = []   # day-offset chain
v2_fail = []   # loss before bound
v3_fail = []   # attritional not strictly after bound
v4_fail = []   # PolicyExpired timing
v5_fail = []   # post-expiry claims
v6_fail = []   # cat fraction consistency
v6_warn = []   # ambiguous multi-LossEvent day

# [1] Day-offset chain
for sid in sub_req_day:
    req_d = sub_req_day[sid]

    if sid in sub_issued_day:
        if sub_issued_day[sid] != req_d:
            v1_fail.append(
                f"  FAIL sub={sid}: LeadQuoteIssued day={sub_issued_day[sid]}"
                f" != LeadQuoteRequested day={req_d}"
            )
        issued_d = sub_issued_day[sid]
    else:
        continue  # later events absent — covered by verify_quoting_flow.py

    if sid in sub_presented_day:
        if sub_presented_day[sid] != issued_d + 1:
            v1_fail.append(
                f"  FAIL sub={sid}: QuotePresented day={sub_presented_day[sid]}"
                f" != LeadQuoteIssued day+1={issued_d + 1}"
            )
        presented_d = sub_presented_day[sid]
    else:
        continue

    if sid in sub_accepted_day:
        if sub_accepted_day[sid] != presented_d:
            v1_fail.append(
                f"  FAIL sub={sid}: QuoteAccepted day={sub_accepted_day[sid]}"
                f" != QuotePresented day={presented_d}"
            )
        accepted_d = sub_accepted_day[sid]
    else:
        continue

    # Skip PolicyBound offset check if accepted on the last day (horizon truncation)
    if accepted_d == max_day:
        continue

    if sid in sub_policy_bound:
        pb_pid, pb_day = sub_policy_bound[sid]
        if pb_day != accepted_d + 1:
            v1_fail.append(
                f"  FAIL sub={sid}: PolicyBound day={pb_day}"
                f" != QuoteAccepted day+1={accepted_d + 1}"
            )

# [2] Loss eligibility — no InsuredLoss before PolicyBound day
for (day, pid, peril, gul) in insured_loss_list:
    if pid not in policy_bound_day:
        continue  # orphan — handled by verify_claims.py
    if day < policy_bound_day[pid]:
        v2_fail.append(
            f"  FAIL day={day} policy={pid} peril={peril}:"
            f" InsuredLoss before PolicyBound day={policy_bound_day[pid]}"
        )

# [3] Attritional InsuredLoss strictly after PolicyBound (day > bound_day)
for (day, pid, peril, gul) in insured_loss_list:
    if peril != "Attritional":
        continue
    if pid not in policy_bound_day:
        continue
    if day <= policy_bound_day[pid]:
        v3_fail.append(
            f"  FAIL day={day} policy={pid}:"
            f" Attritional InsuredLoss not strictly after PolicyBound day={policy_bound_day[pid]}"
        )

# [4] PolicyExpired timing — PolicyExpired.day == PolicyBound.day + 360
for pid in policy_expire_day:
    if pid not in policy_bound_day:
        continue
    expected = policy_bound_day[pid] + 360
    if policy_expire_day[pid] != expected:
        v4_fail.append(
            f"  FAIL policy={pid}: PolicyExpired day={policy_expire_day[pid]}"
            f" != PolicyBound day+360={expected}"
        )

# [5] No post-expiry claims — ClaimSettled.day < PolicyExpired.day
for (day, pid) in claim_settled_list:
    if pid not in policy_expire_day:
        continue  # horizon truncation or orphan
    if day >= policy_expire_day[pid]:
        v5_fail.append(
            f"  FAIL day={day} policy={pid}:"
            f" ClaimSettled on or after PolicyExpired day={policy_expire_day[pid]}"
        )

# [6] Cat damage fraction consistency — shared draw per (WindstormAtlantic, day)
cat_groups_checked = 0
for (peril, day), bucket in insured_losses_by_peril_day.items():
    if peril != "WindstormAtlantic":
        continue
    if loss_events_per_peril_day[(peril, day)] > 1:
        v6_warn.append(
            f"  WARN day={day} peril={peril}: {loss_events_per_peril_day[(peril, day)]}"
            f" LossEvents on same day — fraction consistency check skipped (ambiguous grouping)"
        )
        continue
    if len(bucket) < 2:
        continue  # can't check consistency with a single policy
    cat_groups_checked += 1

    fractions = []
    for (pid, gul) in bucket:
        si = policy_sum_insured.get(pid)
        if si is not None and si > 0:
            fractions.append(gul / si)

    if len(fractions) < 2:
        continue

    # Tolerance: accounts for integer truncation in gul = int(df * si)
    # Use 1/min(si) as the rounding tolerance
    min_si = min(
        policy_sum_insured[pid]
        for (pid, _) in bucket
        if pid in policy_sum_insured and policy_sum_insured[pid] > 0
    )
    tolerance = 1.0 / min_si

    spread = max(fractions) - min(fractions)
    if spread > tolerance:
        v6_fail.append(
            f"  FAIL day={day} peril={peril}:"
            f" damage fraction spread={spread:.6f} > tolerance={tolerance:.6f}"
            f" (min={min(fractions):.6f}, max={max(fractions):.6f},"
            f" n={len(fractions)} policies)"
        )

# ── Output ────────────────────────────────────────────────────────────────────

print(f"Submissions checked (day-offset chain): {len(sub_req_day)}")
print(f"InsuredLoss events checked (loss eligibility + attritional timing): {len(insured_loss_list)}")
print(f"PolicyExpired events checked (timing): {len(policy_expire_day)}")
print(f"ClaimSettled events checked (post-expiry): {len(claim_settled_list)}")
print(f"Cat (day, peril) groups checked (fraction consistency): {cat_groups_checked}")

for w in v6_warn:
    print(w)

all_violations = v1_fail + v2_fail + v3_fail + v4_fail + v5_fail + v6_fail

if all_violations:
    print(f"\nFAIL — {len(all_violations)} violation(s):")
    displayed = 0
    for label, violations in [
        ("[1] Day-offset chain", v1_fail),
        ("[2] Loss eligibility (no pre-bound losses)", v2_fail),
        ("[3] Attritional strictly post-bound", v3_fail),
        ("[4] PolicyExpired timing", v4_fail),
        ("[5] No post-expiry claims", v5_fail),
        ("[6] Cat damage fraction consistency", v6_fail),
    ]:
        if violations:
            print(f"  {label}: {len(violations)} violation(s)")
            for msg in violations:
                if displayed >= 50:
                    remaining = len(all_violations) - 50
                    print(f"  ... and {remaining} more")
                    sys.exit(1)
                print(msg)
                displayed += 1
    sys.exit(1)
else:
    print("\nPASS — all mechanics invariants hold.")
    print("  [1] Day-offset chain: PASS")
    print("  [2] Loss eligibility (no pre-bound losses): PASS")
    print("  [3] Attritional strictly post-bound: PASS")
    print("  [4] PolicyExpired timing: PASS")
    print("  [5] No post-expiry claims: PASS")
    print("  [6] Cat damage fraction consistency: PASS")
