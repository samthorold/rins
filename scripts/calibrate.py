#!/usr/bin/env python3
"""
calibrate.py — measure rins simulation output against Lloyd's market benchmarks.

Run from the project root after `cargo run`:
    python3 scripts/calibrate.py

Benchmarks are derived from:
  - Lloyd's Annual Reports 2018–2023 (combined ratios, GWP, syndicate counts)
  - AM Best / S&P Lloyd's market reviews
  - Lloyds.com market statistics (syndicate count ~80; GWP £44–48bn 2021–2023)

All monetary values in the sim are in pence; benchmarks are expressed as ratios
or relative metrics so absolute scale differences don't affect scoring.

Exit code: 0 if no FAIL metrics, 1 if any FAIL.
"""
import json, sys, collections
from pathlib import Path

EVENTS_FILE = "events.ndjson"

# ── Benchmarks ────────────────────────────────────────────────────────────────
# (metric_key, target_lo, target_hi, warn_lo, warn_hi, unit, description)
# PASS  = value in [target_lo, target_hi]
# WARN  = value in [warn_lo, warn_hi] but outside target
# FAIL  = value outside [warn_lo, warn_hi]
# Use None for unbounded edges.
BENCHMARKS = {
    # Placement: specialty London market places 50–80% of risks presented
    "avg_bind_rate_pct": (
        50, 80, 30, 90, "%",
        "Annual risk placement (bind) rate",
    ),
    # Insolvency: Lloyd's rarely sees >1–2 insolvencies/year; <5% of syndicates
    "avg_annual_insolvency_pct": (
        0, 5, 0, 15, "%",
        "Avg % of active syndicates insolvent each year",
    ),
    "peak_annual_insolvency_pct": (
        0, 10, 0, 25, "%",
        "Worst single-year insolvency rate",
    ),
    # Combined ratio: Lloyd's market 85–105% in normal years; cat years up to ~120%
    # We measure median across all syndicate-years to dampen outlier distortion
    "median_combined_ratio_pct": (
        70, 110, 50, 160, "%",
        "Median syndicate combined ratio (all syndicate-years with premium)",
    ),
    # Worst single syndicate-year: Lloyd's worst individual syndicate ~200% in
    # major cat years; above 300% signals catastrophic miscalibration
    "max_combined_ratio_pct": (
        0, 250, 0, 400, "%",
        "Worst single syndicate-year combined ratio",
    ),
    # Lead concentration: HHI of lead role distribution
    # Real Lloyd's ~80 syndicates, top leads hold 10–25% share → HHI ~200–700
    "lead_hhi": (
        200, 800, 100, 2000, "pts",
        "HHI of lead role distribution (10 000 = monopoly)",
    ),
    # Panel size: Lloyd's risks typically placed with 5–20 syndicates
    "avg_panel_size": (
        5, 15, 3, 25, "synd",
        "Average panel size (syndicates per bound policy)",
    ),
    # Capacity survival: Lloyd's typically loses <5% of capacity per year
    "avg_annual_survival_pct": (
        90, 100, 75, 100, "%",
        "Avg % of syndicates surviving each year",
    ),
    # Follower acceptance: followers should quote most requests (80–95%)
    "avg_follower_acceptance_pct": (
        75, 95, 55, 100, "%",
        "Follower quote acceptance rate",
    ),
}

# Calibration guidance: keyed by metric, ordered by severity of the delta.
# Each entry: (condition_fn, direction, suggestion)
SUGGESTIONS = [
    (
        "avg_annual_insolvency_pct",
        lambda v: v > 15,
        "HIGH insolvency rate — likely causes: cat severity too large or syndicate capital too small. "
        "Try (a) halving `max_loss_fraction` in `PerilConfig` for cat perils in `src/config.rs`, "
        "or (b) doubling the capital floor for small syndicates in `SimulationConfig::canonical`.",
    ),
    (
        "avg_annual_insolvency_pct",
        lambda v: v > 5,
        "Elevated insolvency rate — reduce cat peril severity by ~20% or raise small-syndicate "
        "capital by £30–50 M in `SimulationConfig::canonical`.",
    ),
    (
        "peak_annual_insolvency_pct",
        lambda v: v > 25,
        "Single-year mass-insolvency event — a single cat is wiping out too many syndicates at once. "
        "Check that concentration limits in `Syndicate::can_accept` are enforced; also consider "
        "adding a capital buffer (e.g. 10% of capacity held as undeployable reserve).",
    ),
    (
        "lead_hhi",
        lambda v: v > 2000,
        "Lead role near-monopoly — one syndicate leads almost all risks. "
        "Ensure broker relationship scores are initialised heterogeneously and that "
        "`Broker::select_lead` considers multiple candidates (not just the highest scorer).",
    ),
    (
        "lead_hhi",
        lambda v: v > 800,
        "Lead concentration too high — brokers over-concentrating on a single lead. "
        "Reduce relationship-score stickiness or add a soft diversity penalty in `src/broker.rs`.",
    ),
    (
        "max_combined_ratio_pct",
        lambda v: v > 400,
        "Catastrophic combined ratio in at least one syndicate-year — cat losses are "
        "disproportionate to premiums written. Reduce `severity_scale` or "
        "`mean_loss_pct` in cat `PerilConfig` entries in `src/config.rs`.",
    ),
    (
        "median_combined_ratio_pct",
        lambda v: v > 160,
        "Median combined ratio too high — market is consistently unprofitable. "
        "Either premiums are too low (check `margin_bps` in actuarial params) "
        "or attritional loss rates are too high (check `AttritionalConfig` frequencies).",
    ),
    (
        "median_combined_ratio_pct",
        lambda v: v < 50,
        "Median combined ratio suspiciously low — market may be over-charging. "
        "Check whether claims are being fully attributed (e.g. attritional scheduler firing). "
        "Also check that `ClaimSettled` events are being emitted for all policy types.",
    ),
    (
        "avg_bind_rate_pct",
        lambda v: v < 30,
        "Bind rate too low — most risks are going unplaced. "
        "Syndicates may be too selective. Check acceptance thresholds in `Syndicate::can_accept` "
        "and ensure the leader quote propagates to followers correctly.",
    ),
    (
        "avg_bind_rate_pct",
        lambda v: v > 90,
        "Bind rate too high — nearly every submission is placed. "
        "Market may lack differentiation. Tighten syndicate risk appetite or "
        "increase the diversity of submission risk profiles.",
    ),
    (
        "avg_panel_size",
        lambda v: v < 3,
        "Panels are too small — most risks placed with 1–2 syndicates. "
        "Increase `target_panel_size` in broker config or check follower quoting logic.",
    ),
    (
        "avg_follower_acceptance_pct",
        lambda v: v < 55,
        "Followers declining most requests — follower pricing or capacity logic too restrictive. "
        "Check follower `can_accept` capacity checks and follower margin parameters.",
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
    submissions      = collections.Counter()
    policies         = collections.Counter()
    bound_premiums   = collections.defaultdict(lambda: collections.defaultdict(int))
    claims           = collections.defaultdict(lambda: collections.defaultdict(int))
    panel_sizes      = collections.defaultdict(list)
    lead_freq        = collections.Counter()
    foll_req         = collections.Counter()
    foll_iss         = collections.Counter()
    all_syndicates   = set()
    insolvencies     = collections.defaultdict(set)  # year -> set of syn_ids
    entries_by_year  = collections.defaultdict(set)  # year -> set of syn_ids

    for e in events:
        d, ev = e["day"], e["event"]
        if not isinstance(ev, dict):
            continue
        y = year(d)
        k = next(iter(ev))
        v = ev[k]

        if k == "SyndicateEntered":
            sid = v["syndicate_id"]
            all_syndicates.add(sid)
            entries_by_year[y].add(sid)

        elif k == "SubmissionArrived":
            submissions[y] += 1

        elif k == "PolicyBound":
            policies[y] += 1
            entries = v["panel"]["entries"]
            panel_sizes[y].append(len(entries))
            for entry in entries:
                bound_premiums[y][entry["syndicate_id"]] += entry["premium"]
                if entry.get("is_lead", False) or entries.index(entry) == 0:
                    lead_freq[entry["syndicate_id"]] += 0  # don't double-count; use QuoteIssued

        elif k == "QuoteIssued":
            if v.get("is_lead"):
                lead_freq[v["syndicate_id"]] += 1
            else:
                foll_iss[y] += 1

        elif k == "QuoteRequested":
            if not v.get("is_lead"):
                foll_req[y] += 1

        elif k == "ClaimSettled":
            claims[y][v["syndicate_id"]] += v["amount"]

        elif k == "SyndicateInsolvency":
            insolvencies[y].add(v["syndicate_id"])

    years = sorted(set(submissions) | set(policies))

    # ── Bind rate
    bind_rates = [
        100.0 * policies[y] / submissions[y]
        for y in years if submissions[y] > 0
    ]
    avg_bind_rate = sum(bind_rates) / len(bind_rates) if bind_rates else 0.0

    # ── Insolvency rates
    insolvent_cumulative = set()
    active_start_by_year = {}
    for y in years:
        active_start_by_year[y] = all_syndicates - insolvent_cumulative
        insolvent_cumulative |= insolvencies[y]

    insolvency_pcts = []
    for y in years:
        n_active = len(active_start_by_year[y])
        n_ins    = len(insolvencies[y])
        if n_active > 0:
            insolvency_pcts.append(100.0 * n_ins / n_active)

    avg_insolvency = sum(insolvency_pcts) / len(insolvency_pcts) if insolvency_pcts else 0.0
    peak_insolvency = max(insolvency_pcts) if insolvency_pcts else 0.0

    # ── Survival rate (inverse of insolvency)
    survival_pcts = [100.0 - p for p in insolvency_pcts]
    avg_survival = sum(survival_pcts) / len(survival_pcts) if survival_pcts else 100.0

    # ── Combined ratios (per syndicate per year, using bound premiums)
    combined_ratios = []
    for y in years:
        for syn_id in all_syndicates:
            p = bound_premiums[y].get(syn_id, 0)
            c = claims[y].get(syn_id, 0)
            if p > 0:
                combined_ratios.append(100.0 * c / p)

    combined_ratios.sort()
    median_cr = combined_ratios[len(combined_ratios) // 2] if combined_ratios else 0.0
    max_cr    = combined_ratios[-1] if combined_ratios else 0.0

    # ── Lead HHI
    total_leads = sum(lead_freq.values())
    lead_hhi = (
        sum((cnt / total_leads) ** 2 for cnt in lead_freq.values()) * 10000
        if total_leads > 0 else 10000.0
    )

    # ── Panel size
    all_panel_sizes = [sz for y in years for sz in panel_sizes[y]]
    avg_panel = sum(all_panel_sizes) / len(all_panel_sizes) if all_panel_sizes else 0.0

    # ── Follower acceptance
    foll_acc_pcts = [
        100.0 * foll_iss[y] / foll_req[y]
        for y in years if foll_req[y] > 0
    ]
    avg_foll_acc = sum(foll_acc_pcts) / len(foll_acc_pcts) if foll_acc_pcts else 0.0

    return {
        "years": years,
        "avg_bind_rate_pct":          avg_bind_rate,
        "bind_rates_by_year":         list(zip(years, bind_rates)),
        "avg_annual_insolvency_pct":  avg_insolvency,
        "peak_annual_insolvency_pct": peak_insolvency,
        "insolvency_pcts_by_year":    list(zip(years, insolvency_pcts)),
        "avg_annual_survival_pct":    avg_survival,
        "median_combined_ratio_pct":  median_cr,
        "max_combined_ratio_pct":     max_cr,
        "lead_hhi":                   lead_hhi,
        "avg_panel_size":             avg_panel,
        "avg_follower_acceptance_pct": avg_foll_acc,
        # detail
        "n_syndicates":               len(all_syndicates),
        "lead_top5":                  lead_freq.most_common(5),
    }

# ── Scoring ────────────────────────────────────────────────────────────────────
def score(value, bm):
    target_lo, target_hi, warn_lo, warn_hi = bm[0], bm[1], bm[2], bm[3]
    if target_lo <= value <= target_hi:
        return "PASS"
    if warn_lo <= value <= warn_hi:
        return "WARN"
    return "FAIL"

def delta_str(value, bm):
    target_lo, target_hi = bm[0], bm[1]
    if value < target_lo:
        return f"{value - target_lo:+.1f} (below target)"
    if value > target_hi:
        return f"{value - target_hi:+.1f} (above target)"
    return "—"

# ── Report ─────────────────────────────────────────────────────────────────────
def print_report(metrics):
    print("=" * 72)
    print("CALIBRATION REPORT — rins vs Lloyd's market benchmarks")
    print("=" * 72)
    print(f"  Syndicates:    {metrics['n_syndicates']}")
    print(f"  Years:         {metrics['years']}")
    print()

    status_order = {"FAIL": 0, "WARN": 1, "PASS": 2}
    rows = []
    for key, bm in BENCHMARKS.items():
        value = metrics[key]
        s     = score(value, bm)
        unit  = bm[4]
        desc  = bm[5]
        rows.append((status_order[s], s, key, value, unit, desc, bm))

    rows.sort(key=lambda r: r[0])

    col_w = 46
    print(f"  {'Metric':<{col_w}}  {'Value':>8}  {'Target':>14}  {'Status'}")
    print(f"  {'-'*col_w}  {'-'*8}  {'-'*14}  {'-'*6}")
    for _, s, key, value, unit, desc, bm in rows:
        target_lo, target_hi = bm[0], bm[1]
        target_str = f"{target_lo}–{target_hi} {unit}"
        val_str    = f"{value:.1f} {unit}"
        marker     = {"PASS": "✓", "WARN": "△", "FAIL": "✗"}[s]
        print(f"  {desc:<{col_w}}  {val_str:>10}  {target_str:>14}  {marker} {s}")

    # ── Year-by-year detail
    print()
    print("── Bind rate by year ────────────────────────────────────────────────")
    for y, r in metrics["bind_rates_by_year"]:
        bar = "█" * int(r / 5)
        print(f"  Y{y}: {r:5.1f}%  {bar}")

    print()
    print("── Insolvency rate by year ──────────────────────────────────────────")
    for y, r in metrics["insolvency_pcts_by_year"]:
        bar = "█" * int(r / 5)
        print(f"  Y{y}: {r:5.1f}%  {bar}")

    print()
    print("── Lead role top-5 syndicates ───────────────────────────────────────")
    total_leads = sum(c for _, c in metrics["lead_top5"])
    for sid, cnt in metrics["lead_top5"]:
        share = 100.0 * cnt / total_leads if total_leads else 0
        bar   = "█" * int(share / 5)
        print(f"  Syn {sid:>3}: {cnt:>5} leads  ({share:.1f}%)  {bar}")

    # ── Suggestions
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
            label = BENCHMARKS[metric_key][5]
            status = score(value, BENCHMARKS[metric_key])
            print(f"\n  [{rank}] {label}  ({value:.1f} {BENCHMARKS[metric_key][4]})  [{status}]")
            # word-wrap at 68 chars
            words  = text.split()
            line   = "      "
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

    # exit code
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
