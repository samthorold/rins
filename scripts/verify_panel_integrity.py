#!/usr/bin/env python3
"""
verify_panel_integrity.py — event-stream verifier for PolicyBound panel structure.

Invariants:
  - Every PolicyBound panel must have entries summing to exactly 10,000 bps.
  - Every panel entry must have share_bps > 0.

Run from the project root after `cargo run`:
    python3 scripts/verify_panel_integrity.py
"""
import json, sys
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

failures = []
checked = 0

for e in events:
    ev = e["event"]
    if not isinstance(ev, dict): continue
    k = next(iter(ev))
    if k != "PolicyBound": continue
    v = ev[k]
    day = e["day"]
    sid = v["submission_id"]
    entries = v["panel"]["entries"]
    checked += 1

    total_bps = sum(entry["share_bps"] for entry in entries)
    if total_bps != 10_000:
        failures.append(f"  FAIL submission_id={sid} day={day}: panel sums to {total_bps} bps (expected 10,000)")

    for entry in entries:
        if entry["share_bps"] <= 0:
            failures.append(
                f"  FAIL submission_id={sid} day={day}: "
                f"syndicate_id={entry['syndicate_id']} has share_bps={entry['share_bps']}"
            )

print(f"PolicyBound panels checked: {checked}")

if failures:
    print(f"\nFAIL — {len(failures)} violation(s):")
    for f in failures[:50]:
        print(f)
    if len(failures) > 50:
        print(f"  ... and {len(failures) - 50} more")
    sys.exit(1)
else:
    print("\nPASS — all panels sum to 10,000 bps with positive entries.")
