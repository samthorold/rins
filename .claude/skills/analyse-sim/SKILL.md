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

*Claim: attritional LR is stable at market scale and more volatile at insurer scale, confirming the LLN pooling benefit. Cat LR is zero in quiet years and spikes sharply in cat years — structurally different from attritional.*

Compute from `analyse_sim.py` output:

1. **Identify pure quiet years** — years with zero `LossEvent`s (from the "Loss events per year" table).

2. **Market attritional CV** — take market-level attritional LR% for each quiet year (from "Market-level Attritional vs Cat loss ratio" table, `AttrLR%` column). Compute CV = std_dev / mean across those years.

3. **Per-insurer attritional CV** — in pure quiet years, each insurer's total LR equals their attritional LR (no cat). From the "Per-insurer loss ratio" table, take each insurer's LR for those years, compute their individual CV, then take the **median** CV across all insurers.

4. **Cat contrast** — confirm cat LR = 0% in all quiet years AND cat LR ≥ 50% in at least one cat year (from "Market-level Attritional vs Cat loss ratio" table, `CatLR%` column).

**Verdict thresholds:**
- **CONFIRMED** — market attritional CV < 0.35 AND market CV < median insurer CV AND cat contrast holds
- **PARTIAL** — market attritional CV < 0.35 but market CV ≥ median insurer CV (LLN visible at market scale but scale contrast absent), OR market CV < median insurer CV but cat contrast weak
- **NOT VISIBLE** — market attritional CV ≥ 0.35 (loss ratio too volatile at market scale to claim stability)

Report: market CV, median insurer CV, the ratio (market CV / median insurer CV), and the peak cat LR year.

---

#### §2 Catastrophe-Amplified Capital Crisis — `[PARTIAL]`

*Claim: cat events drive simultaneous losses across all insurers large enough to breach 100% LR; insolvency processing is not yet active so the full cascade cannot occur, but the capital impact is landing correctly.*

1. Identify severe years (market LR ≥ 100%) that are cat-driven (cat GUL% > 50% of total GUL).
2. In each such year, confirm all five insurers have LR > 100% (shared occurrence, not idiosyncratic).
3. Note final insurer capitals from simulation stdout — confirm they are negative, reflecting unprocessed accumulated losses.

**Verdict thresholds:**
- **PARTIAL CONFIRMED** — at least one cat-driven severe year with all insurers breaching 100% LR; final capitals negative
- **NOT VISIBLE** — no cat-driven severe year, or insurers show divergent LRs in a cat year (routing bug)

---

#### §8 Geographic and Peril Accumulation Risk — `[PARTIAL]`

*Claim: in cat years, the shared occurrence hits all insurers simultaneously; per-insurer LR spread in cat years is narrower than in attritional-only years, confirming correlated (not independent) exposure.*

1. In each cat year, note the range (max − min) of per-insurer LR across all five insurers.
2. In pure quiet years, note the same range.
3. Confirm: median insurer LR range in cat years < median range in quiet years (shared cat shock compresses divergence; attritional is independent across policies → wider spread).

**Verdict thresholds:**
- **PARTIAL CONFIRMED** — insurer LR range in cat years is materially narrower than in quiet years; cross-territory contrast not yet measurable (single territory model)
- **NOT VISIBLE** — insurer LR range in cat years is as wide or wider than in quiet years

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
