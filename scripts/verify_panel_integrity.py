#!/usr/bin/env python3
"""
verify_panel_integrity.py — event-stream verifier for bind flow integrity.

Invariants:
  1. Every QuoteAccepted (not on the final simulated day) has exactly one
     downstream PolicyBound. A QuoteAccepted on the last day is exempt because
     PolicyBound fires +1 day later and falls outside the simulation window.
  2. PolicyBound.insurer_id matches the LeadQuoteIssued.insurer_id for that
     submission.
  3. Every PolicyExpired.policy_id was previously bound (appears in PolicyBound).
  4. No duplicate PolicyBound for the same policy_id.

Run from the project root after `cargo run`:
    python3 scripts/verify_panel_integrity.py
"""
import json, sys
from collections import Counter
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

failures = []

max_day = max(e["day"] for e in events)

# Pass 1: collect LeadQuoteIssued insurer per submission
quote_insurer = {}   # submission_id -> insurer_id
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]
    if k == "LeadQuoteIssued":
        quote_insurer[v["submission_id"]] = v["insurer_id"]

# Pass 2: collect QuoteAccepted, PolicyBound, PolicyExpired
quote_accepted = {}              # submission_id -> day (only non-final-day ones)
bound_per_submission = Counter() # submission_id -> count
bound_insurer = {}               # submission_id -> insurer_id at bind time
bound_policy_ids = set()         # all policy_ids seen in PolicyBound

for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k == "QuoteAccepted":
        # PolicyBound fires day+1; if this is the last day, it won't appear
        if day < max_day:
            quote_accepted[v["submission_id"]] = day
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

# Check 1: each non-truncated QuoteAccepted has exactly one PolicyBound
for sid, acc_day in quote_accepted.items():
    n = bound_per_submission.get(sid, 0)
    if n != 1:
        failures.append(
            f"  FAIL submission_id={sid} (QuoteAccepted day={acc_day}): "
            f"expected 1 PolicyBound, got {n}"
        )

# Check 2: PolicyBound insurer_id matches LeadQuoteIssued insurer_id
for sid, bound_iid in bound_insurer.items():
    quoted_iid = quote_insurer.get(sid)
    if quoted_iid is None:
        failures.append(f"  FAIL submission_id={sid}: PolicyBound but no LeadQuoteIssued found")
    elif quoted_iid != bound_iid:
        failures.append(
            f"  FAIL submission_id={sid}: LeadQuoteIssued insurer={quoted_iid} "
            f"but PolicyBound insurer={bound_iid}"
        )

checked = len(quote_accepted)
print(f"QuoteAccepted events checked (excl. final-day truncation): {checked}")
print(f"PolicyBound events checked: {sum(bound_per_submission.values())}")
print(f"PolicyExpired events referencing valid policy_id: checked inline")
print(f"(max_day={max_day})")

if failures:
    print(f"\nFAIL — {len(failures)} violation(s):")
    for f in failures[:50]:
        print(f)
    if len(failures) > 50:
        print(f"  ... and {len(failures) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — bind flow is coherent: every submission binds once with the correct insurer.")
