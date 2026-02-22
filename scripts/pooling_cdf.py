"""
pooling_cdf.py — Risk Pooling: the insured's perspective.

Risk pooling is a benefit to *insureds*. Each individual faces uncertain
annual losses — a wide distribution with most years modest and rare years
catastrophic. Insurance lets the insured swap that uncertain outcome for a
fixed premium. The insurer can offer this exchange because the Law of Large
Numbers makes *aggregate* losses predictable as pool size grows.

The attritional and catastrophe components behave differently:
  • Attritional: losses are independent across insureds → pooling compresses
    aggregate variance efficiently (CV ~ 1/√N).
  • Cat: a single shared occurrence strikes all exposed risks simultaneously
    → adding more risks in the same territory provides little diversification.

Two-panel output:
  Left  — Attritional: individual insured annual GUL (wide, orange) vs
           market-average GUL per insured (narrow, blue) vs premium (dashed).
  Right — Cat years only: same comparison; the two CDFs are much closer
           together, showing pooling fails for correlated losses.

Usage:
    python3 scripts/pooling_cdf.py              # 50 seeds, save to docs/
    python3 scripts/pooling_cdf.py --seeds 20   # quick iteration
    python3 scripts/pooling_cdf.py --no-plot    # summary stats only
"""

import argparse
import json
import math
import os
import statistics
import subprocess
import sys
import tempfile
from collections import defaultdict

BINARY = os.path.join(os.path.dirname(__file__), "..", "target", "release", "rins")
PROJECT_ROOT = os.path.join(os.path.dirname(__file__), "..")
OUTPUT_DIR = os.path.join(os.path.dirname(__file__), "..", "docs")
OUTPUT_FILE = os.path.join(OUTPUT_DIR, "pooling_cdf.png")

ASSET_VALUE_CENTS = 5_000_000_000  # 50M USD in cents


# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
def build():
    print("Building release binary...", flush=True)
    result = subprocess.run(
        ["cargo", "build", "--release"],
        cwd=PROJECT_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print("BUILD FAILED:\n" + result.stderr, file=sys.stderr)
        sys.exit(1)
    print("  Build OK.")


# ---------------------------------------------------------------------------
# Simulation run
# ---------------------------------------------------------------------------
def run_seed(seed: int, output_path: str) -> bool:
    result = subprocess.run(
        [BINARY, "--seed", str(seed), "--output", output_path, "--quiet"],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0


def parse_events(path: str) -> list:
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


# ---------------------------------------------------------------------------
# Data extraction
# ---------------------------------------------------------------------------
def extract_data(events: list):
    """
    Returns:
        attr_gul  : {year: {insured_id: total_attritional_gul_cents}}
        cat_gul   : {year: {insured_id: total_cat_gul_cents}}
        cat_years : set of years with ≥1 LossEvent
        active    : {year: set(insured_id)} — insureds with bound policies
        premium   : representative premium per insured (cents)
    """
    cat_years: set[int] = set()
    attr_gul: dict = defaultdict(lambda: defaultdict(int))
    cat_gul: dict = defaultdict(lambda: defaultdict(int))
    active: dict = defaultdict(set)
    premiums: list[int] = []

    for e in events:
        day: int = e["day"]
        ev = e["event"]
        if not isinstance(ev, dict):
            continue

        year = day // 360 + 1

        if "LossEvent" in ev:
            cat_years.add(year)

        elif "InsuredLoss" in ev:
            il = ev["InsuredLoss"]
            iid = il["insured_id"]
            gul = il["ground_up_loss"]
            if il["peril"] == "Attritional":
                attr_gul[year][iid] += gul
            else:
                cat_gul[year][iid] += gul

        elif "PolicyBound" in ev:
            pb = ev["PolicyBound"]
            active[year].add(pb["insured_id"])
            premiums.append(pb["premium"])

    premium = statistics.mode(premiums) if premiums else 0
    return attr_gul, cat_gul, cat_years, active, premium


# ---------------------------------------------------------------------------
# Aggregation across seeds
# ---------------------------------------------------------------------------
def collect_observations(
    attr_gul, cat_gul, cat_years, active, skip_year1: bool = True
) -> tuple:
    """
    Returns four lists of values (as % of asset value):
        ind_attr  — individual insured annual attritional GUL
        mkt_attr  — market-average attritional GUL per insured (one per year)
        ind_cat   — individual insured annual cat GUL (cat years only)
        mkt_cat   — market-average cat GUL per insured (cat years only)
    """
    ind_attr, mkt_attr = [], []
    ind_cat, mkt_cat = [], []

    all_years = set(active.keys())
    if skip_year1:
        all_years.discard(1)

    for year in all_years:
        insured_ids = active[year]
        if not insured_ids:
            continue
        n = len(insured_ids)

        # Attritional: every active insured, including those with 0 attritional loss
        year_attr = attr_gul.get(year, {})
        for iid in insured_ids:
            ind_attr.append(year_attr.get(iid, 0) / ASSET_VALUE_CENTS * 100)
        total_attr = sum(year_attr.get(iid, 0) for iid in insured_ids)
        mkt_attr.append(total_attr / n / ASSET_VALUE_CENTS * 100)

        # Cat (cat years only)
        if year in cat_years:
            year_cat = cat_gul.get(year, {})
            for iid in insured_ids:
                ind_cat.append(year_cat.get(iid, 0) / ASSET_VALUE_CENTS * 100)
            total_cat = sum(year_cat.get(iid, 0) for iid in insured_ids)
            mkt_cat.append(total_cat / n / ASSET_VALUE_CENTS * 100)

    return ind_attr, mkt_attr, ind_cat, mkt_cat


# ---------------------------------------------------------------------------
# Empirical CDF
# ---------------------------------------------------------------------------
def ecdf(values: list[float]) -> tuple[list[float], list[float]]:
    xs = sorted(values)
    n = len(xs)
    ps = [(i + 1) / n for i in range(n)]
    return xs, ps


# ---------------------------------------------------------------------------
# Summary stats
# ---------------------------------------------------------------------------
def cv(values: list[float]) -> float:
    if len(values) < 2:
        return float("nan")
    m = statistics.mean(values)
    return statistics.pstdev(values) / m if m else float("nan")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--seeds", type=int, default=50)
    parser.add_argument("--seed-start", type=int, default=1)
    parser.add_argument("--no-plot", action="store_true")
    args = parser.parse_args()

    build()

    all_ind_attr: list[float] = []
    all_mkt_attr: list[float] = []
    all_ind_cat: list[float] = []
    all_mkt_cat: list[float] = []
    premium_pct: float | None = None
    failed = 0

    print(f"\nRunning {args.seeds} seeds (start={args.seed_start})...", flush=True)

    with tempfile.NamedTemporaryFile(suffix=".ndjson", delete=False) as tmp:
        tmp_path = tmp.name

    try:
        for seed in range(args.seed_start, args.seed_start + args.seeds):
            if not run_seed(seed, tmp_path):
                failed += 1
                continue

            events = parse_events(tmp_path)
            attr_gul, cat_gul, cat_years, active, premium = extract_data(events)

            if premium_pct is None and premium > 0:
                premium_pct = premium / ASSET_VALUE_CENTS * 100

            ia, ma, ic, mc = collect_observations(attr_gul, cat_gul, cat_years, active)
            all_ind_attr.extend(ia)
            all_mkt_attr.extend(ma)
            all_ind_cat.extend(ic)
            all_mkt_cat.extend(mc)

            print(
                f"  Seed {seed:3d}: {len(ma)} years, {len(mc)} cat-years",
                flush=True,
            )
    finally:
        os.unlink(tmp_path)

    if failed:
        print(f"\nWARN: {failed} seeds failed and were skipped.")

    # --- Summary ---
    cv_ind_attr = cv(all_ind_attr)
    cv_mkt_attr = cv(all_mkt_attr)
    cv_ind_cat = cv(all_ind_cat) if all_ind_cat else float("nan")
    cv_mkt_cat = cv(all_mkt_cat) if all_mkt_cat else float("nan")
    n_insureds = 100  # canonical

    print(f"\n=== Risk Pooling — Insured's Perspective ({args.seeds} seeds) ===")
    print(f"  Premium per insured: {premium_pct:.1f}% of asset value")
    print()
    print("  Attritional (independent losses):")
    print(f"    Individual GUL:      mean={statistics.mean(all_ind_attr):.1f}%  CV={cv_ind_attr:.3f}  n={len(all_ind_attr)}")
    print(f"    Market average/ins.: mean={statistics.mean(all_mkt_attr):.1f}%  CV={cv_mkt_attr:.3f}  n={len(all_mkt_attr)}")
    print(f"    CV ratio (ind/mkt):  {cv_ind_attr/cv_mkt_attr:.2f}×  (LLN predicts √{n_insureds}={math.sqrt(n_insureds):.1f}×)")
    print()
    if all_ind_cat:
        print("  Cat (correlated losses, cat years only):")
        print(f"    Individual GUL:      mean={statistics.mean(all_ind_cat):.1f}%  CV={cv_ind_cat:.3f}  n={len(all_ind_cat)}")
        print(f"    Market average/ins.: mean={statistics.mean(all_mkt_cat):.1f}%  CV={cv_mkt_cat:.3f}  n={len(all_mkt_cat)}")
        print(f"    CV ratio (ind/mkt):  {cv_ind_cat/cv_mkt_cat:.2f}×  (pooling provides less diversification)")

    if args.no_plot:
        return

    try:
        import matplotlib.pyplot as plt
        import matplotlib.ticker as mticker
    except ImportError:
        print("\nmatplotlib not available — install with: pip install matplotlib")
        return

    fig, axes = plt.subplots(1, 2, figsize=(13, 6))
    fig.suptitle(
        f"Risk Pooling: Insured's Perspective  ({args.seeds} seeds × 20 yr)",
        fontsize=13,
    )

    # --- Left panel: Attritional ---
    ax = axes[0]
    xs, ps = ecdf(all_ind_attr)
    ax.plot(xs, ps, color="darkorange", linewidth=1.8, alpha=0.85,
            label=f"Individual insured  CV={cv_ind_attr:.2f}")
    xs, ps = ecdf(all_mkt_attr)
    ax.plot(xs, ps, color="steelblue", linewidth=2.5,
            label=f"Market average/insured  CV={cv_mkt_attr:.2f}")
    if premium_pct is not None:
        ax.axvline(premium_pct, color="black", linewidth=1.2, linestyle="--",
                   label=f"Premium  ({premium_pct:.0f}% of asset value)")
    ax.set_title("Attritional losses — pooling works\n"
                 f"CV ratio {cv_ind_attr/cv_mkt_attr:.1f}× (LLN predicts √{n_insureds}={math.sqrt(n_insureds):.0f}×)",
                 fontsize=10)
    ax.set_xlabel("Annual attritional GUL (% of asset value)", fontsize=10)
    ax.set_ylabel("Empirical CDF", fontsize=10)
    ax.xaxis.set_major_formatter(mticker.PercentFormatter(decimals=0))
    ax.yaxis.set_major_formatter(mticker.PercentFormatter(xmax=1, decimals=0))
    ax.legend(fontsize=9, loc="lower right")
    ax.grid(True, alpha=0.3)

    # --- Right panel: Cat ---
    ax = axes[1]
    if all_ind_cat:
        xs, ps = ecdf(all_ind_cat)
        ax.plot(xs, ps, color="darkorange", linewidth=1.8, alpha=0.85,
                label=f"Individual insured  CV={cv_ind_cat:.2f}")
        xs, ps = ecdf(all_mkt_cat)
        ax.plot(xs, ps, color="steelblue", linewidth=2.5,
                label=f"Market average/insured  CV={cv_mkt_cat:.2f}")
        ax.set_title(
            "Cat losses (cat years only) — pooling breaks down\n"
            f"CV ratio {cv_ind_cat/cv_mkt_cat:.1f}× (shared occurrence, little diversification)",
            fontsize=10,
        )
    else:
        ax.text(0.5, 0.5, "No cat years observed", ha="center", va="center",
                transform=ax.transAxes)
        ax.set_title("Cat losses — no data", fontsize=10)
    ax.set_xlabel("Annual cat GUL (% of asset value)", fontsize=10)
    ax.set_ylabel("Empirical CDF", fontsize=10)
    ax.xaxis.set_major_formatter(mticker.PercentFormatter(decimals=0))
    ax.yaxis.set_major_formatter(mticker.PercentFormatter(xmax=1, decimals=0))
    ax.legend(fontsize=9, loc="lower right")
    ax.grid(True, alpha=0.3)

    fig.tight_layout()
    os.makedirs(OUTPUT_DIR, exist_ok=True)
    fig.savefig(OUTPUT_FILE, dpi=150)
    print(f"\nPlot saved to: {OUTPUT_FILE}")
    plt.show()


if __name__ == "__main__":
    main()
