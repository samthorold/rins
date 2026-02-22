#!/usr/bin/env python3
"""
verify_panel_integrity.py — event-stream verifier for bind flow integrity.

Replaces the old panel-based checks (no panels in this model).

Invariants:
  1. Every SubmissionArrived has exactly one downstream PolicyBound.
  2. PolicyBound.insurer_id matches the QuoteIssued.insurer_id for that submission.
  3. Every PolicyExpired.policy_id was previously bound (appears in PolicyBound).
  4. No duplicate PolicyBound for the same policy_id.

Run from the project root after `cargo run`:
    python3 scripts/verify_panel_integrity.py
"""
import json, sys
from collections import defaultdict, Counter
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

failures = []

# Pass 1: collect QuoteIssued insurer per submission
quote_insurer = {}   # submission_id -> insurer_id
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "QuoteIssued":
        quote_insurer[v["submission_id"]] = v["insurer_id"]

# Pass 2: collect PolicyBound per submission and track policy_ids
bound_per_submission = Counter()     # submission_id -> count
bound_insurer = {}                   # submission_id -> insurer_id at bind time
bound_policy_ids = set()             # all policy_ids seen in PolicyBound
submission_arrived = set()

for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k == "SubmissionArrived":
        submission_arrived.add(v["submission_id"])
    elif k == "PolicyBound":
        sid = v["submission_id"]
        pid = v["policy_id"]
        iid = v["insurer_id"]
        bound_per_submission[sid] += 1
        bound_insurer[sid] = iid
        if pid in bound_policy_ids:
            failures.append(f"  FAIL day={day}: duplicate PolicyBound for policy_id={pid}")
        bound_policy_ids.add(pid)
    elif k == "PolicyExpired":
        pid = v["policy_id"]
        if pid not in bound_policy_ids:
            failures.append(f"  FAIL day={day}: PolicyExpired for unknown policy_id={pid}")

# Check 1: each SubmissionArrived has exactly one PolicyBound
for sid in submission_arrived:
    n = bound_per_submission.get(sid, 0)
    if n != 1:
        failures.append(
            f"  FAIL submission_id={sid}: expected 1 PolicyBound, got {n}"
        )

# Check 2: PolicyBound insurer_id matches QuoteIssued insurer_id
for sid, bound_iid in bound_insurer.items():
    quoted_iid = quote_insurer.get(sid)
    if quoted_iid is None:
        failures.append(f"  FAIL submission_id={sid}: PolicyBound but no QuoteIssued found")
    elif quoted_iid != bound_iid:
        failures.append(
            f"  FAIL submission_id={sid}: QuoteIssued insurer={quoted_iid} "
            f"but PolicyBound insurer={bound_iid}"
        )

checked = len(submission_arrived)
print(f"SubmissionArrived events checked: {checked}")
print(f"PolicyBound events checked: {sum(bound_per_submission.values())}")
print(f"PolicyExpired events referencing valid policy_id: checked inline")

if failures:
    print(f"\nFAIL — {len(failures)} violation(s):")
    for f in failures[:50]:
        print(f)
    if len(failures) > 50:
        print(f"  ... and {len(failures) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — bind flow is coherent: every submission binds once with the correct insurer.")
