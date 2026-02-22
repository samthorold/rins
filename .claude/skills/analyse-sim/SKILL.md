---
name: analyse-sim
description: Regenerate the rins simulation and produce a structured year-over-year analysis of events.ndjson
user-invocable: true
allowed-tools: Bash
---

Follow these steps exactly. Do not skip regeneration.

## Step 1 — Regenerate

Run `cargo run --release` in /Users/sam/Projects/rins to produce a fresh events.ndjson.
Report any build or runtime errors and stop if the run fails.

> **Maintenance note:** Mechanics checks in `scripts/verify_mechanics.py` are derived from
> the `[ACTIVE]` sections of `docs/market-mechanics.md`. When that document changes — new
> `[ACTIVE]` sections added, existing ones promoted from `[PARTIAL]`, or invariants revised —
> review and update `scripts/verify_mechanics.py` to match.

## Step 2 — Analyse and verify

From /Users/sam/Projects/rins, run `verify_mechanics.py` first. If it FAILs, report mechanics violations prominently before proceeding.

```
python3 scripts/verify_mechanics.py
python3 scripts/analyse_sim.py
python3 scripts/verify_claims.py
python3 scripts/verify_insolvency.py
python3 scripts/verify_panel_integrity.py
python3 scripts/verify_quoting_flow.py
```

Report any FAIL lines from each verifier before the Step 3 analysis.

## Step 3 — Report

Structure the report as four explicit priority tiers. Work top-to-bottom; if Tier 1 has critical failures, note that deeper tiers may be unreliable.

---

### Tier 1 — Mechanics & Verifier Status (always)

List each of the 6 mechanics invariants as **PASS** or **FAIL** (from `verify_mechanics.py` output).
List each secondary verifier as **PASS** or **FAIL**:
- `verify_claims.py`
- `verify_insolvency.py`
- `verify_panel_integrity.py`
- `verify_quoting_flow.py`

If any invariant or verifier FAILs: name it and its violation count prominently.
If any WARN appears: flag it as an unusual run signal.
If critical failures exist, note that Tiers 2–4 may be unreliable and stop.

---

### Tier 2 — Year Character Table (always)

Produce one row per year:

| Year | Tag | Market LR% | Dominant Peril | Worst Insurer LR% |
|------|-----|------------|----------------|-------------------|

**Tag thresholds:**
- **quiet** — no cat `LossEvent` AND market LR < 70%
- **moderate** — cat present but market LR < 100%, OR no cat but LR 70–100%
- **severe** — market LR ≥ 100%

**Dominant peril:** "Attritional" if Cat GUL% < 30%, "Mixed" if 30–60%, "Cat" if > 60%.

After the table, note the count of quiet / moderate / severe years.

---

### Tier 3 — Stress Deep-Dive (only if any year is tagged severe)

For each severe year:
- What triggered it: number of cat `LossEvent`s and total cat GUL
- Which insurer had the worst LR, and which large insured(s) drove that concentration
- Top insured GUL driver that year and their share of total GUL
- Pattern: is stress worsening over time (trend), or random cat-driven spikes?

Skip this tier entirely if no severe year exists.

---

### Tier 4 — One Investigation Question (always)

One sharp, specific question tied to the most striking data signal from Tiers 2–3.
It must reference a specific number or pattern from this run — not a generic prompt.

Good examples:
- "Insurer 4 holds the top-3 cat-exposed large insureds in years 7 and 20 — is round-robin creating systematic concentration?"
- "AttrLR runs 60–64% in benign years — is the attritional rate parameter calibrated correctly?"
