#!/usr/bin/env python3
"""
Plot per-year cross-seed distributions from rins simulation.

Usage:
    python3 scripts/plot_distributions.py [--runs N] [--csv path] [--out path]

Generates a fan-chart figure: median line, shaded IQR (p25–p75), and lighter
p5–p95 band, for LossR%, Rate%, CombR%, and TotalCap(B USD).
"""

import argparse
import subprocess
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches


# ── CLI ───────────────────────────────────────────────────────────────────────

parser = argparse.ArgumentParser()
parser.add_argument("--runs", type=int, default=50)
parser.add_argument("--csv", default=None, help="Use existing CSV instead of re-running")
parser.add_argument("--out", default="dist_plot.png")
parser.add_argument("--seed", type=int, default=None)
parser.add_argument("--years", type=int, default=None)
args = parser.parse_args()

# ── Data ──────────────────────────────────────────────────────────────────────

if args.csv:
    csv_path = args.csv
else:
    csv_path = "/tmp/rins_runs.csv"
    cmd = ["cargo", "run", "--release", "--", f"--runs", str(args.runs),
           "--csv", csv_path, "--quiet"]
    if args.seed is not None:
        cmd += ["--seed", str(args.seed)]
    if args.years is not None:
        cmd += ["--years", str(args.years)]
    print(f"Running: {' '.join(cmd)}", file=sys.stderr)
    subprocess.run(cmd, check=True)

df = pd.read_csv(csv_path)
df["loss_ratio_pct"]    = df["loss_ratio"]    * 100
df["combined_ratio_pct"] = df["combined_ratio"] * 100
df["rate_on_line_pct"]  = df["rate_on_line"]  * 100

years = sorted(df["year"].unique())

# ── Quantile fan chart helper ─────────────────────────────────────────────────

def fan(ax, df, col, color, label, ymax=None):
    g = df.groupby("year")[col]
    p5  = g.quantile(0.05)
    p25 = g.quantile(0.25)
    p50 = g.quantile(0.50)
    p75 = g.quantile(0.75)
    p95 = g.quantile(0.95)
    xs  = p50.index

    ax.fill_between(xs, p5,  p95, alpha=0.18, color=color, linewidth=0)
    ax.fill_between(xs, p25, p75, alpha=0.35, color=color, linewidth=0)
    ax.plot(xs, p50, color=color, linewidth=2, label=label)

    if ymax is not None:
        ax.set_ylim(bottom=0, top=ymax)
    else:
        ax.set_ylim(bottom=0)

    return p5, p25, p50, p75, p95


# ── Discrete count bar helper ─────────────────────────────────────────────────

def bar_p50_max(ax, df, col, color_p50, color_max, label):
    g = df.groupby("year")[col]
    p50 = g.median()
    mx  = g.max()
    xs  = p50.index
    width = 0.35
    ax.bar(xs - width/2, p50, width, color=color_p50, alpha=0.8, label=f"{label} p50")
    ax.bar(xs + width/2, mx,  width, color=color_max,  alpha=0.4, label=f"{label} max")
    ax.set_ylim(bottom=0)


# ── Figure ────────────────────────────────────────────────────────────────────

n_runs = df["seed"].nunique()
fig, axes = plt.subplots(3, 2, figsize=(14, 13))
fig.suptitle(f"rins — cross-seed distributions through time  (N={n_runs} runs)", fontsize=13)

# Shared x-axis setup
for ax in axes.flat:
    ax.set_xlabel("Year")
    ax.set_xticks(years[::2])
    ax.grid(axis="y", linewidth=0.4, alpha=0.5)
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)

legend_patches = [
    mpatches.Patch(alpha=0.9, label="p50 (median)"),
    mpatches.Patch(alpha=0.35, label="IQR (p25–p75)"),
    mpatches.Patch(alpha=0.18, label="p5–p95"),
]

# ── Row 0: LossR% and CombR% ─────────────────────────────────────────────────

ax = axes[0, 0]
fan(ax, df, "loss_ratio_pct", "#d62728", "LossR%")
ax.axhline(100, color="black", linewidth=0.8, linestyle="--", alpha=0.4)
ax.set_ylabel("Loss ratio (%)")
ax.set_title("Loss Ratio")
ax.legend(handles=legend_patches, fontsize=8, loc="upper left")

ax = axes[0, 1]
fan(ax, df, "combined_ratio_pct", "#9467bd", "CombR%")
ax.axhline(100, color="black", linewidth=0.8, linestyle="--", alpha=0.4)
ax.set_ylabel("Combined ratio (%)")
ax.set_title("Combined Ratio")

# ── Row 1: Rate% and TotalCap ─────────────────────────────────────────────────

ax = axes[1, 0]
fan(ax, df, "rate_on_line_pct", "#1f77b4", "Rate%")
ax.set_ylabel("Rate on line (%)")
ax.set_title("Market Rate on Line")

ax = axes[1, 1]
fan(ax, df, "total_cap_b", "#2ca02c", "TotalCap")
ax.set_ylabel("Total market capital (B USD)")
ax.set_title("Total Capital")

# ── Row 2: Cat events and Dropped# ───────────────────────────────────────────

ax = axes[2, 0]
bar_p50_max(ax, df, "cat_events", "#ff7f0e", "#ffbb78", "Cat events")
ax.set_ylabel("Cat events per year")
ax.set_title("Catastrophe Events")
ax.legend(fontsize=8)

ax = axes[2, 1]
bar_p50_max(ax, df, "dropped_count", "#8c564b", "#c49c94", "Dropped")
ax.set_ylabel("Submissions dropped")
ax.set_title("Capacity Shortfall (Dropped Submissions)")
ax.legend(fontsize=8)

fig.tight_layout()
out = args.out
fig.savefig(out, dpi=150, bbox_inches="tight")
print(f"Saved: {out}")
