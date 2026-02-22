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

### Tier 1.5 — Phenomena Check (always)

Assess each phenomenon currently tagged `[EMERGING]` or `[PARTIAL]` in `docs/phenomena.md`. Use data already produced by `analyse_sim.py`; no new scripts required. Deliver a one-line verdict per phenomenon.

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
    elif 'InsuredLoss' in ev:
        il = ev['InsuredLoss']
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

*Claim: cat events drive simultaneous losses across all insurers large enough to breach 100% LR; insolvency processing is not yet active so the full cascade cannot occur, but the capital impact is landing correctly.*

1. Identify severe years (market LR ≥ 100%) that are cat-driven (cat GUL% > 50% of total GUL).
2. In each such year, confirm all five insurers have LR > 100% (shared occurrence, not idiosyncratic).
3. Note that capital resets each YearStart, so final capitals will be positive — the crisis manifests as within-year LR breaches, not terminal insolvency.

**Verdict thresholds:**
- **PARTIAL CONFIRMED** — at least one cat-driven severe year with all insurers breaching 100% LR
- **NOT VISIBLE** — no cat-driven severe year, or insurers show divergent LRs in a cat year (routing bug)

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
