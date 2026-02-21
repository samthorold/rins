#!/usr/bin/env python3
"""
verify_insolvency.py — event-stream verifier for SyndicateInsolvency uniqueness.

Invariant: each syndicate must have at most one SyndicateInsolvency event.

Run from the project root after `cargo run`:
    python3 scripts/verify_insolvency.py
"""
import json, sys
from collections import Counter
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

counts = Counter()
for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev))
    if k == "SyndicateInsolvency":
        counts[ev[k]["syndicate_id"]] += 1

violations = {sid: n for sid, n in counts.items() if n > 1}

total_insolvent = len(counts)
print(f"Syndicates declared insolvent: {total_insolvent}")

if violations:
    print(f"\nFAIL — {len(violations)} syndicate(s) have duplicate SyndicateInsolvency events:")
    for sid, n in sorted(violations.items()):
        print(f"  syndicate_id={sid}: {n} events")
    sys.exit(1)
else:
    print("\nPASS — each syndicate has at most one SyndicateInsolvency event.")
