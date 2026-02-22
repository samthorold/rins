#!/usr/bin/env python3
"""
verify_quoting_flow.py — event-stream verifier for quoting flow coherence.

Invariant: every LeadQuoteRequested for (submission_id, insurer_id) must have
exactly one response (LeadQuoteIssued or QuoteRejected). No responses without a
prior request.

Run from the project root after `cargo run`:
    python3 scripts/verify_quoting_flow.py
"""
import json, sys
from collections import defaultdict
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

requested = {}    # (submission_id, insurer_id) -> day
responses = defaultdict(list)  # (submission_id, insurer_id) -> [event_type, ...]

for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev)); v = ev[k]; day = e["day"]
    if k == "LeadQuoteRequested":
        key = (v["submission_id"], v["insurer_id"])
        requested[key] = day
    elif k in ("LeadQuoteIssued", "QuoteRejected"):
        key = (v["submission_id"], v["insurer_id"])
        responses[key].append(k)

failures = []

# Orphan requests (zero responses)
for key, req_day in sorted(requested.items()):
    if key not in responses:
        sub_id, ins_id = key
        failures.append(
            f"  FAIL submission_id={sub_id} insurer_id={ins_id}: "
            f"LeadQuoteRequested on day={req_day} has no response"
        )

# Duplicate responses
for key, resp_list in sorted(responses.items()):
    if len(resp_list) > 1:
        sub_id, ins_id = key
        failures.append(
            f"  FAIL submission_id={sub_id} insurer_id={ins_id}: "
            f"{len(resp_list)} responses ({', '.join(resp_list)})"
        )

# Responses without a prior request
for key, resp_list in sorted(responses.items()):
    if key not in requested:
        sub_id, ins_id = key
        failures.append(
            f"  FAIL submission_id={sub_id} insurer_id={ins_id}: "
            f"response ({resp_list[0]}) has no prior LeadQuoteRequested"
        )

print(f"LeadQuoteRequested pairs checked: {len(requested)}")
print(f"Responses received: {sum(len(r) for r in responses.values())}")

if failures:
    print(f"\nFAIL — {len(failures)} violation(s):")
    for f in failures[:50]:
        print(f)
    if len(failures) > 50:
        print(f"  ... and {len(failures) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — every LeadQuoteRequested has exactly one response; no orphan responses.")
