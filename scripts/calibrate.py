#!/usr/bin/env python3
"""
calibrate.py — measure rins simulation output against Lloyd's market benchmarks.

Run from the project root after `cargo run`:
    python3 scripts/calibrate.py

Benchmarks are derived from:
  - Lloyd's Annual Reports 2018–2023 (combined ratios, GWP, syndicate counts)
  - AM Best / S&P Lloyd's market reviews

All monetary values in the sim are in cents; benchmarks are expressed as ratios
or relative metrics so absolute scale differences don't affect scoring.

Exit code: 0 if no FAIL metrics, 1 if any FAIL.
"""
import json, sys, collections, statistics
from pathlib import Path

EVENTS_FILE = "events.ndjson"

# ── Benchmarks ────────────────────────────────────────────────────────────────
# (target_lo, target_hi, warn_lo, warn_hi, unit, description)
# PASS  = value in [target_lo, target_hi]
# WARN  = value in [warn_lo, warn_hi] but outside target
# FAIL  = value outside [warn_lo, warn_hi]
BENCHMARKS = {
    # Placement: all risks place in this model (always-accept insured/insurer).
    # Lloyd's reality is 50–80%; this gap signals the model needs selective underwriting.
    "avg_bind_rate_pct": (
        95, 100, 80, 100, "%",
        "Annual risk placement (bind) rate",
    ),
    # Combined ratio: Lloyd's market 85–105% in normal years; cat years up to ~120%.
    # Median across all insurer-years to dampen single-year cat distortion.
    "median_combined_ratio_pct": (
        70, 110, 50, 160, "%",
        "Median insurer combined ratio (all insurer-years with premium)",
    ),
    # Worst single insurer-year.
    "max_combined_ratio_pct": (
        0, 250, 0, 400, "%",
        "Worst single insurer-year combined ratio",
    ),
    # Quiet-year combined ratio: should be well below 100% so cat years are
    # financed by quiet-year profit. Lloyd's target ~80–90% in non-cat years.
    "quiet_year_median_cr_pct": (
        50, 95, 30, 120, "%",
        "Median combined ratio in non-cat years",
    ),
    # Attritional loss ratio: Lloyd's attritional ~30–50% of GWP in normal years.
    "avg_attritional_lr_pct": (
        20, 55, 10, 80, "%",
        "Avg attritional claims / total premium across years",
    ),
}

# Calibration guidance keyed by metric.
SUGGESTIONS = [
    (
        "median_combined_ratio_pct",
        lambda v: v > 160,
        "Median combined ratio too high — market is consistently unprofitable. "
        "Increase `rate` in `InsurerConfig` in `src/config.rs` (e.g. 0.03 instead of 0.02), "
        "or reduce attritional frequency (`annual_rate`) or severity (`mu`, `sigma`).",
    ),
    (
        "median_combined_ratio_pct",
        lambda v: v < 50,
        "Median combined ratio suspiciously low — premiums far exceed claims. "
        "Check that `ClaimSettled` events are being emitted (verify attritional scheduler fires) "
        "and that rate is not set unrealistically high.",
    ),
    (
        "max_combined_ratio_pct",
        lambda v: v > 400,
        "A single insurer-year has a catastrophic combined ratio — cat losses overwhelm premiums. "
        "Reduce cat frequency (`annual_frequency`) or cat severity (`pareto_scale` / `pareto_shape`) "
        "in `CatConfig` in `src/config.rs`, or increase insurer `rate`.",
    ),
    (
        "quiet_year_median_cr_pct",
        lambda v: v > 120,
        "Even non-cat years are unprofitable — attritional load is too heavy relative to rate. "
        "Reduce `annual_rate` in `AttritionalConfig` or lower severity (`mu`) to cut attritional GUL.",
    ),
    (
        "avg_attritional_lr_pct",
        lambda v: v > 80,
        "Attritional loss ratio extremely high — attritional claims dominate. "
        "Lower `annual_rate` in `AttritionalConfig` or reduce mean damage fraction (`mu`).",
    ),
    (
        "avg_attritional_lr_pct",
        lambda v: v < 10,
        "Attritional loss ratio near zero — attritional losses barely register. "
        "Check that the attritional scheduler fires at `PolicyBound` "
        "and that `annual_rate` is non-zero in `AttritionalConfig`.",
    ),
]

# ── Load and parse ─────────────────────────────────────────────────────────────

def load_events():
    return [json.loads(l) for l in Path(EVENTS_FILE).read_text().splitlines() if l.strip()]

def year(day):
    return day // 360 + 1

def etype(e):
    ev = e["event"]
    return next(iter(ev)) if isinstance(ev, dict) else ev

# ── Metric extraction ──────────────────────────────────────────────────────────

def extract_metrics(events):
    submissions     = collections.Counter()   # year -> count CoverageRequested
    policies        = collections.Counter()   # year -> count PolicyBound
    premiums        = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> insurer -> cents
    claims          = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> insurer -> cents
    claims_attr     = collections.Counter()   # year -> attritional cents
    all_insurers    = set()
    loss_event_years = set()

    # submission_id -> insurer_id (from LeadQuoteIssued)
    sub_insurer = {}
    # submission_id -> premium (from QuoteAccepted)
    sub_premium = {}

    for e in events:
        d, ev = e["day"], e["event"]
        if not isinstance(ev, dict):
            continue
        y = year(d)
        k = next(iter(ev))
        v = ev[k]

        if k == "CoverageRequested":
            submissions[y] += 1

        elif k == "LeadQuoteIssued":
            sub_insurer[v["submission_id"]] = v["insurer_id"]

        elif k == "QuoteAccepted":
            sub_premium[v["submission_id"]] = v["premium"]

        elif k == "PolicyBound":
            policies[y] += 1
            sid = v["submission_id"]
            iid = v["insurer_id"]
            all_insurers.add(iid)
            prem = sub_premium.get(sid, 0)
            premiums[y][iid] += prem

        elif k == "LossEvent":
            if v.get("peril") not in ("Attritional",):
                loss_event_years.add(y)

        elif k == "ClaimSettled":
            iid = v["insurer_id"]
            all_insurers.add(iid)
            claims[y][iid] += v["amount"]
            if v.get("peril") == "Attritional":
                claims_attr[y] += v["amount"]

    years = sorted(set(submissions) | set(policies))
    cat_years = loss_event_years
    quiet_years = [y for y in years if y not in cat_years]

    # Bind rate
    bind_rates = [
        100.0 * policies[y] / submissions[y]
        for y in years if submissions[y] > 0
    ]
    avg_bind_rate = statistics.mean(bind_rates) if bind_rates else 0.0

    # Combined ratios per insurer per year
    combined_ratios = []
    quiet_combined_ratios = []
    for y in years:
        for iid in all_insurers:
            p = premiums[y].get(iid, 0)
            c = claims[y].get(iid, 0)
            if p > 0:
                cr = 100.0 * c / p
                combined_ratios.append(cr)
                if y in quiet_years:
                    quiet_combined_ratios.append(cr)

    combined_ratios.sort()
    median_cr = statistics.median(combined_ratios) if combined_ratios else 0.0
    max_cr    = max(combined_ratios) if combined_ratios else 0.0
    quiet_median_cr = statistics.median(quiet_combined_ratios) if quiet_combined_ratios else 0.0

    # Attritional LR per year
    attr_lrs = []
    for y in years:
        total_prem = sum(premiums[y].values())
        if total_prem > 0:
            attr_lrs.append(100.0 * claims_attr[y] / total_prem)
    avg_attr_lr = statistics.mean(attr_lrs) if attr_lrs else 0.0

    # Implied breakeven rate (what rate × sum_insured would give 65% LR)
    total_prem_all = sum(sum(premiums[y].values()) for y in years)
    total_claims_all = sum(sum(claims[y].values()) for y in years)
    actual_lr = total_claims_all / total_prem_all if total_prem_all > 0 else 0.0

    return {
        "years": years,
        "cat_years": sorted(cat_years),
        "quiet_years": sorted(quiet_years),
        "n_insurers": len(all_insurers),
        "avg_bind_rate_pct":          avg_bind_rate,
        "bind_rates_by_year":         list(zip(years, bind_rates)),
        "median_combined_ratio_pct":  median_cr,
        "max_combined_ratio_pct":     max_cr,
        "quiet_year_median_cr_pct":   quiet_median_cr,
        "avg_attritional_lr_pct":     avg_attr_lr,
        "actual_lr":                  actual_lr,
        # Per-year detail
        "premiums_by_year":           {y: sum(premiums[y].values()) for y in years},
        "claims_by_year":             {y: sum(claims[y].values()) for y in years},
        "attr_claims_by_year":        dict(claims_attr),
    }

# ── Scoring ────────────────────────────────────────────────────────────────────

def score(value, bm):
    lo, hi, wlo, whi = bm[0], bm[1], bm[2], bm[3]
    if lo <= value <= hi:
        return "PASS"
    if wlo <= value <= whi:
        return "WARN"
    return "FAIL"

# ── Report ─────────────────────────────────────────────────────────────────────

def print_report(metrics):
    print("=" * 72)
    print("CALIBRATION REPORT — rins vs Lloyd's market benchmarks")
    print("=" * 72)
    print(f"  Insurers : {metrics['n_insurers']}")
    print(f"  Years    : {metrics['years']}")
    print(f"  Cat years: {metrics['cat_years']}")
    print()

    status_order = {"FAIL": 0, "WARN": 1, "PASS": 2}
    rows = []
    for key, bm in BENCHMARKS.items():
        value = metrics[key]
        s     = score(value, bm)
        rows.append((status_order[s], s, key, value, bm))

    rows.sort(key=lambda r: r[0])

    col_w = 52
    print(f"  {'Metric':<{col_w}}  {'Value':>8}  {'Target':>14}  {'Status'}")
    print(f"  {'-'*col_w}  {'-'*8}  {'-'*14}  {'-'*6}")
    for _, s, key, value, bm in rows:
        lo, hi, _, _, unit, desc = bm
        target_str = f"{lo}–{hi} {unit}"
        val_str    = f"{value:.1f} {unit}"
        marker     = {"PASS": "✓", "WARN": "△", "FAIL": "✗"}[s]
        print(f"  {desc:<{col_w}}  {val_str:>10}  {target_str:>14}  {marker} {s}")

    # Year-by-year detail
    print()
    print("── Loss ratio by year (claims / premiums) ───────────────────────────")
    print(f"  {'Year':>4}  {'Cat?':>5}  {'Premiums':>16}  {'Claims':>16}  {'LR%':>7}  {'AttrLR%':>8}")
    for y in metrics["years"]:
        prem  = metrics["premiums_by_year"].get(y, 0)
        clm   = metrics["claims_by_year"].get(y, 0)
        attr  = metrics["attr_claims_by_year"].get(y, 0)
        lr    = 100.0 * clm / prem if prem else 0.0
        alr   = 100.0 * attr / prem if prem else 0.0
        flag  = " CAT" if y in metrics["cat_years"] else "    "
        print(f"  {y:>4}  {flag:>5}  {prem:>16,}  {clm:>16,}  {lr:>7.1f}  {alr:>8.1f}")

    overall_lr = 100.0 * metrics["actual_lr"]
    target_lr  = 65.0
    implied_rate_adj = overall_lr / target_lr  # multiply current rate by this
    print()
    print(f"  Overall LR (all years): {overall_lr:.1f}%")
    print(f"  To reach {target_lr:.0f}% LR, scale `rate` by ×{implied_rate_adj:.2f}")
    print(f"  (e.g. current rate=0.02 → implied breakeven rate={0.02 * implied_rate_adj:.4f})")

    # Suggestions
    print()
    print("=" * 72)
    print("TOP CALIBRATION SUGGESTIONS")
    print("=" * 72)

    fired = []
    for metric_key, cond, text in SUGGESTIONS:
        value = metrics.get(metric_key)
        if value is not None and cond(value):
            status = score(value, BENCHMARKS[metric_key])
            priority = 0 if status == "FAIL" else 1
            fired.append((priority, metric_key, value, text))

    fired.sort(key=lambda x: x[0])

    if not fired:
        print("  All metrics within target range — no urgent changes needed.")
    else:
        shown = set()
        rank  = 1
        for priority, metric_key, value, text in fired:
            if metric_key in shown:
                continue
            shown.add(metric_key)
            label  = BENCHMARKS[metric_key][5]
            status = score(value, BENCHMARKS[metric_key])
            unit   = BENCHMARKS[metric_key][4]
            print(f"\n  [{rank}] {label}  ({value:.1f} {unit})  [{status}]")
            words = text.split()
            line  = "      "
            for w in words:
                if len(line) + len(w) + 1 > 70:
                    print(line)
                    line = "      " + w + " "
                else:
                    line += w + " "
            if line.strip():
                print(line)
            rank += 1
            if rank > 3:
                break

    print()

    any_fail = any(score(metrics[k], bm) == "FAIL" for k, bm in BENCHMARKS.items())
    return 1 if any_fail else 0

# ── Entry point ────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    try:
        events = load_events()
    except FileNotFoundError:
        print(f"ERROR: {EVENTS_FILE} not found. Run `cargo run` first.", file=sys.stderr)
        sys.exit(2)

    metrics = extract_metrics(events)
    rc = print_report(metrics)
    sys.exit(rc)
