#!/usr/bin/env python3
"""
verify_quoting_flow.py — event-stream verifier for quoting flow coherence.

Invariant: every LeadQuoteRequested for (submission_id, insurer_id) must have
exactly one response (LeadQuoteIssued, LeadQuoteDeclined, or QuoteRejected).
No responses without a prior request.

Run from the project root after `cargo run`:
    python3 scripts/verify_quoting_flow.py
"""
import sys
import os; sys.path.insert(0, os.path.dirname(__file__))
from event_index import build_index

idx = build_index()
requested = idx.quote_requested
responses = idx.sub_responses

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

print(f"LeadQuoteRequested pairs checked: {len(idx.quote_requested)}")
print(f"Responses received: {sum(len(r) for r in idx.sub_responses.values())}")

if failures:
    print(f"\nFAIL — {len(failures)} violation(s):")
    for f in failures[:50]:
        print(f)
    if len(failures) > 50:
        print(f"  ... and {len(failures) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — every LeadQuoteRequested has exactly one response (Issued/Declined/Rejected); no orphan responses.")
