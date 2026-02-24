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

## Step 2 — Analyse and verify

From /Users/sam/Projects/rins, run the Rust analyser (Tier 1 mechanics + integrity + Tier 2 table).
If any Tier 1 invariant FAILs, report violations prominently before proceeding.

```
cargo run --release --bin analyse 2>&1
```

The Rust analyser covers all 18 invariants (Inv 1–6 mechanics, Inv 7–18 integrity).
`verify_claims.py`, `verify_insolvency.py`, `verify_panel_integrity.py`, and `verify_quoting_flow.py`
have been deleted — their checks are part of the Rust Tier 1 output.

Report any FAIL lines from the Rust analyser before the Step 3 analysis.

## Step 3 — Report

Structure the report as four explicit priority tiers. Work top-to-bottom; if Tier 1 has critical failures, note that deeper tiers may be unreliable.

---

### Tier 1 — Mechanics & Verifier Status (always)

List each of the 18 invariants as **PASS** or **FAIL** (from `cargo run --release --bin analyse` output):

**Mechanics (Inv 1–6):**
- Inv 1 — Day offset chain
- Inv 2 — No loss before bound
- Inv 3 — Attritional strictly post-bound
- Inv 4 — PolicyExpired timing
- Inv 5 — No claim after expiry
- Inv 6 — Cat GUL ≤ sum insured

**Integrity (Inv 7–18):**
- Inv 7 — GUL ≤ sum insured (all perils)
- Inv 8 — Aggregate claim ≤ sum insured per (policy, year)
- Inv 9 — Every ClaimSettled has matching InsuredLoss
- Inv 10 — Claim amount > 0
- Inv 11 — ClaimSettled insurer matches PolicyBound insurer
- Inv 12 — Every QuoteAccepted (non-final-day) has PolicyBound
- Inv 13 — PolicyBound insurer matches LeadQuoteIssued insurer
- Inv 14 — No duplicate PolicyBound for same policy_id
- Inv 15 — Every PolicyExpired references a bound policy
- Inv 16 — Every LeadQuoteRequested has exactly one insurer response
- Inv 17 — No duplicate insurer responses for same (submission, insurer)
- Inv 18 — Every insurer response has a prior LeadQuoteRequested

If any invariant FAILs: name it and its violation count prominently.
If any WARN appears: flag it as an unusual run signal.
If critical failures exist, note that Tiers 2–4 may be unreliable and stop.

---

### Tier 1.5 — Phenomena Check (always)

Assess each phenomenon currently tagged `[EMERGING]` or `[PARTIAL]` in `docs/phenomena.md`. Use data already produced by the Rust analyser and the inline snippets below; no additional scripts required. Deliver a one-line verdict per phenomenon.

> **Maintenance note:** When a badge changes in `docs/phenomena.md` (PLANNED → PARTIAL → EMERGING), add or update the corresponding check here. Remove a check only when the phenomenon is so well-established it no longer warrants active monitoring.

---

#### §0 Risk Pooling — `[EMERGING]`

*Claim: insurance benefits insureds by exchanging uncertain individual losses (high CV) for a fixed premium. LLN makes this possible — aggregate attritional losses are predictable (CV ~ 1/√N). Cat losses are correlated; pooling within one territory provides no variance reduction (cat CV ratio ≈ 1×).*

Run this inline snippet against the already-generated `events.ndjson`:

```python
import json, statistics, math
from collections import defaultdict

ASSET = 5_000_000_000
events = [json.loads(l) for l in open("events.ndjson") if l.strip()]
cat_years = set()
attr_gul = defaultdict(lambda: defaultdict(int))
cat_gul  = defaultdict(lambda: defaultdict(int))
active   = defaultdict(set)

for e in events:
    day, ev = e['day'], e['event']
    if not isinstance(ev, dict): continue
    y = day // 360 + 1
    if 'LossEvent'   in ev: cat_years.add(y)
    elif 'AssetDamage' in ev:
        il = ev['AssetDamage']
        (attr_gul if il['peril'] == 'Attritional' else cat_gul)[y][il['insured_id']] += il['ground_up_loss']
    elif 'PolicyBound' in ev:
        active[y].add(ev['PolicyBound']['insured_id'])

ind_attr, mkt_attr, ind_cat, mkt_cat = [], [], [], []
for y in sorted(active):
    if y == 1: continue   # skip staggered startup year
    ids = active[y]; n = len(ids)
    for iid in ids: ind_attr.append(attr_gul[y].get(iid, 0) / ASSET * 100)
    mkt_attr.append(sum(attr_gul[y].get(i, 0) for i in ids) / n / ASSET * 100)
    if y in cat_years:
        for iid in ids: ind_cat.append(cat_gul[y].get(iid, 0) / ASSET * 100)
        mkt_cat.append(sum(cat_gul[y].get(i, 0) for i in ids) / n / ASSET * 100)

def cv(v): m = statistics.mean(v); return statistics.pstdev(v) / m
n_ins = round(statistics.mean(len(active[y]) for y in active if y != 1))
cv_ia, cv_ma = cv(ind_attr), cv(mkt_attr)
print(f"Attritional  ind CV={cv_ia:.2f}  mkt CV={cv_ma:.2f}  ratio={cv_ia/cv_ma:.1f}x  (LLN √{n_ins}={math.sqrt(n_ins):.0f}x)")
if ind_cat:
    cv_ic, cv_mc = cv(ind_cat), cv(mkt_cat)
    print(f"Cat          ind CV={cv_ic:.2f}  mkt CV={cv_mc:.2f}  ratio={cv_ic/cv_mc:.1f}x  ({len(mkt_cat)} cat-year obs)")
```

**Verdict thresholds:**
- **CONFIRMED** — attritional CV ratio > 5× AND cat CV ratio < 3× (pooling works for independent losses, fails for correlated)
- **PARTIAL** — attritional CV ratio > 5× but cat CV ratio ≥ 3× (attritional pooling visible but cat contrast weak), OR attritional CV ratio 2–5× (some pooling but below expected LLN scale)
- **NOT VISIBLE** — attritional CV ratio < 2× (individual and market losses are similarly volatile; pooling not operating)

Report: individual attritional CV, market attritional CV, CV ratio, LLN prediction (√N), cat CV ratio, number of cat-year observations.

---

#### §2 Catastrophe-Amplified Capital Crisis — `[PARTIAL]`

*Claim: cat events drive simultaneous losses across all insurers large enough to breach 100% LR. Capital is persistent (no annual re-endowment) and floors at zero — a severe cat year can trigger insolvency and permanently erode market capacity.*

1. Identify severe years (market LR ≥ 100%) that are cat-driven (Dominant Peril = Cat or Mixed) from the Tier 2 table.
2. In each such year, confirm all five insurers have LR > 100% (shared occurrence, not idiosyncratic).
3. Check TotalCap(B) trend around severe years — a drop confirms capital erosion is landing. Check Insolvent# for any terminal failures.

**Verdict thresholds:**
- **PARTIAL CONFIRMED** — at least one cat-driven severe year with all insurers breaching 100% LR
- **NOT VISIBLE** — no cat-driven severe year, or insurers show divergent LRs in a cat year (routing bug)

---

### Tier 2 — Year Character Table (always)

Reproduce the full table exactly as printed by `cargo run --release --bin analyse` — header line,
separator line, and all data rows — inside a fenced code block. Do not reformat, subset, or
convert to a markdown table.

```
(paste raw output here)
```

**LossR%:** pure loss ratio = total claims / total gross premium.

**CombR%:** combined ratio = LossR% + expense ratio (34.4%). Underwriting profit requires CombR% < 100%; above 100% the market is loss-making before investment income.

**Rate%:** market-wide rate on line = bound premium / sum insured. Varies year to year as the attritional EWMA updates ATP. A declining trend over benign years indicates EWMA softening. Flag any year where Rate% falls below the earliest-year Rate% by more than 0.5pp as a soft-market signal.

**Dominant peril:** "Attritional" if Cat GUL% < 30%, "Mixed" if 30–60%, "Cat" if > 60%.

**TotalCap(B):** total capital across all insurers at year-end (billions USD, floored at 0 per insurer). A declining trend flags capital erosion from cumulative losses.

**Insolvent#:** count of `InsurerInsolvent` events in the year. Any non-zero value should be noted explicitly.

After the table, note any year with Insolvent# > 0 and call out years where CombR% > 100% (market loss-making).

---

### Tier 3 — Stress Deep-Dive (only if any year has LossR% ≥ 100%)

For each loss-making year (LossR% ≥ 100%):
- What triggered it: number of cat `LossEvent`s and total cat GUL
- Top insured GUL driver that year and their share of total GUL
- Whether claims exceeded the actuarial floor: LossR% > ~95% implies ATP adequacy > 1.0, since premium ≈ ATP × 1.10 (5% profit loading + ~5% cycle adj floor)
- Pattern: is stress worsening over time (trend), or random cat-driven spikes?

Skip this tier entirely if no year has LossR% ≥ 100%.

---

### Tier 4 — One Investigation Question (always)

One sharp, specific question tied to the most striking data signal from Tiers 2–3.
It must reference a specific number or pattern from this run — not a generic prompt.

Good examples:
- "Insurer 4 holds the top-3 cat-exposed large insureds in years 7 and 20 — is round-robin creating systematic concentration?"
- "AttrLR runs 60–64% in benign years — is the attritional rate parameter calibrated correctly?"
