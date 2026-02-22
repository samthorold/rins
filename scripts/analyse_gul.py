#!/usr/bin/env python3
"""
analyse_gul.py — raw ground-up loss (GUL) analysis, independent of insurance mechanics.

Studies the most basic market layer: insured assets × perils → GUL.
Policy terms, panel splitting, and claims settlement are deliberately excluded.

Run from the project root after `cargo run`:
    python3 scripts/analyse_gul.py
"""
import json, collections, statistics
from pathlib import Path

# ── Load ──────────────────────────────────────────────────────────────────────

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

def year(day): return day // 360 + 1
def etype(e):  return next(iter(e['event'])) if isinstance(e['event'], dict) else e['event']
def pence_to_gbp(p): return p / 100

CAT_PERILS = {"WindstormAtlantic"}

# ── Pass 1: build insured metadata from SubmissionArrived ────────────────────
# Each insured submits once per year; all submissions carry the same fixed risk.
# We use the first submission per insured to capture territory and sum_insured.

insured_territory = {}   # insured_id (int) -> territory (str)
insured_si        = {}   # insured_id (int) -> sum_insured (pence)

for e in events:
    if etype(e) == "SubmissionArrived":
        d = e['event']['SubmissionArrived']
        iid = d['insured_id']
        if iid not in insured_territory:
            risk = d['risk']
            insured_territory[iid] = risk['territory']
            insured_si[iid]        = risk['sum_insured']

# ── Pass 2: collect cat events (to label cat-years) ───────────────────────────

cat_events_by_year = collections.defaultdict(list)  # year -> [(peril, region)]
for e in events:
    if etype(e) == "LossEvent":
        d = e['event']['LossEvent']
        y = year(e['day'])
        cat_events_by_year[y].append((d['peril'],))

cat_years = {y for y, evs in cat_events_by_year.items() if any(p in CAT_PERILS for (p,) in evs)}

# ── Pass 3: collect InsuredLoss events ────────────────────────────────────────
#
# InsuredLoss.ground_up_loss is the raw physical demand per occurrence.
# Multiple events in the same year can sum to more than sum_insured (e.g. a
# building that floods twice). The effective annual GUL is capped at sum_insured,
# matching the cap applied in Market::on_insured_loss at ClaimSettled time.
#
# We track both raw demand and effective GUL so the script can report either.

# Raw demand (pre-cap), indexed [year][insured_id]
raw_gul_by_year_insured   = collections.defaultdict(lambda: collections.defaultdict(int))

# Effective GUL (capped at SI), indexed [year][insured_id]
gul_by_year_insured       = collections.defaultdict(lambda: collections.defaultdict(int))
# Remaining asset value per (insured, year) — reset each year
remaining_si              = {}  # (iid, year) -> remaining pence

gul_by_year_peril         = collections.defaultdict(lambda: collections.defaultdict(int))
gul_by_year_territory     = collections.defaultdict(lambda: collections.defaultdict(int))
gul_by_year_type          = collections.defaultdict(lambda: collections.defaultdict(int))

gul_detail = collections.defaultdict(lambda: collections.defaultdict(lambda: collections.defaultdict(int)))
damage_fractions_by_peril = collections.defaultdict(list)

insured_loss_count = 0

for e in events:
    if etype(e) != "InsuredLoss":
        continue
    d     = e['event']['InsuredLoss']
    iid   = d['insured_id']
    peril = d['peril']
    raw   = d['ground_up_loss']
    y     = year(e['day'])
    insured_loss_count += 1

    si = insured_si.get(iid, 0)
    key = (iid, y)
    if key not in remaining_si:
        remaining_si[key] = si
    effective = min(raw, remaining_si[key])
    remaining_si[key] -= effective

    raw_gul_by_year_insured[y][iid]              += raw
    gul_by_year_insured[y][iid]                  += effective
    gul_by_year_peril[y][peril]                  += effective
    territory = insured_territory.get(iid, "Unknown")
    gul_by_year_territory[y][territory]           += effective
    loss_type = "Attritional" if peril == "Attritional" else "Cat"
    gul_by_year_type[y][loss_type]               += effective
    gul_detail[iid][y][peril]                    += effective

    if si > 0:
        damage_fractions_by_peril[peril].append(raw / si)

all_years = sorted(set(gul_by_year_insured) | set(cat_events_by_year))

# ── Helpers ───────────────────────────────────────────────────────────────────

def fmt_gbp(pence):
    gbp = pence_to_gbp(pence)
    if gbp >= 1_000_000:
        return f"£{gbp/1_000_000:.2f}M"
    if gbp >= 1_000:
        return f"£{gbp/1_000:.1f}K"
    return f"£{gbp:.0f}"

def percentile(data, p):
    if not data: return 0
    data = sorted(data)
    idx = (len(data) - 1) * p / 100
    lo, hi = int(idx), min(int(idx) + 1, len(data) - 1)
    return data[lo] + (data[hi] - data[lo]) * (idx - lo)

def pct_str(num, denom):
    if denom == 0: return " n/a"
    return f"{100*num/denom:5.1f}%"

# ── Section 1: Event inventory ────────────────────────────────────────────────

print("=" * 70)
print("SECTION 1 — EVENT INVENTORY")
print("=" * 70)
print(f"  InsuredLoss events total : {insured_loss_count:,}")
print(f"  Insured assets catalogued: {len(insured_territory):,}")
print(f"  Simulation years         : {min(all_years)}–{max(all_years)}" if all_years else "  (no years)")
print(f"  Cat-active years         : {sorted(cat_years)}")
print()

# ── Section 2: Annual aggregate GUL ───────────────────────────────────────────

print("=" * 70)
print("SECTION 2 — ANNUAL AGGREGATE GUL (all insureds, all perils)")
print("=" * 70)
print(f"  {'Year':>4}  {'Cat?':>5}  {'Total GUL':>12}  {'Cat GUL':>12}  {'Attrit GUL':>12}  {'Attrit%':>7}")
print(f"  {'-'*4}  {'-'*5}  {'-'*12}  {'-'*12}  {'-'*12}  {'-'*7}")
for y in all_years:
    total   = sum(gul_by_year_insured[y].values())
    cat_gul  = gul_by_year_type[y].get("Cat", 0)
    attr_gul = gul_by_year_type[y].get("Attritional", 0)
    flag = " CAT" if y in cat_years else "    "
    print(f"  {y:>4}  {flag:>5}  {fmt_gbp(total):>12}  {fmt_gbp(cat_gul):>12}  {fmt_gbp(attr_gul):>12}  {pct_str(attr_gul, total):>7}")
print()

# ── Section 3: GUL by territory, per year ─────────────────────────────────────

print("=" * 70)
print("SECTION 3 — ANNUAL GUL BY TERRITORY")
print("=" * 70)
territories = sorted({t for yd in gul_by_year_territory.values() for t in yd})
header = f"  {'Year':>4}  {'Cat?':>5}" + "".join(f"  {t:>12}" for t in territories) + f"  {'Total':>12}"
print(header)
print("  " + "-" * (len(header) - 2))
for y in all_years:
    flag = " CAT" if y in cat_years else "    "
    row = f"  {y:>4}  {flag:>5}"
    total = 0
    for t in territories:
        v = gul_by_year_territory[y].get(t, 0)
        total += v
        row += f"  {fmt_gbp(v):>12}"
    row += f"  {fmt_gbp(total):>12}"
    print(row)
print()

# ── Section 4: GUL by peril, per year ─────────────────────────────────────────

print("=" * 70)
print("SECTION 4 — ANNUAL GUL BY PERIL")
print("=" * 70)
perils = sorted({p for yd in gul_by_year_peril.values() for p in yd})
header = f"  {'Year':>4}  {'Cat?':>5}" + "".join(f"  {p[:12]:>12}" for p in perils)
print(header)
print("  " + "-" * (len(header) - 2))
for y in all_years:
    flag = " CAT" if y in cat_years else "    "
    row = f"  {y:>4}  {flag:>5}"
    for p in perils:
        v = gul_by_year_peril[y].get(p, 0)
        row += f"  {fmt_gbp(v):>12}"
    print(row)
print()

# ── Section 5: Per-insured annual GUL distribution ────────────────────────────

print("=" * 70)
print("SECTION 5 — PER-INSURED ANNUAL GUL DISTRIBUTION (across all years)")
print("  Effective GUL = raw demand capped at sum_insured per (insured, year).")
print("  Raw demand shown separately to expose excess scheduling.")
print("=" * 70)

all_insured_ids = set(insured_territory)

def insured_year_observations(gul_dict):
    """Return list of (insured, year, gul) for all insured-years."""
    return [gul_dict[y].get(iid, 0) for y in all_years for iid in all_insured_ids]

eff_obs = insured_year_observations(gul_by_year_insured)
raw_obs = insured_year_observations(raw_gul_by_year_insured)

eff_nonzero = [v for v in eff_obs if v > 0]
raw_nonzero = [v for v in raw_obs if v > 0]
pct_loss_years = 100 * len(eff_nonzero) / len(eff_obs) if eff_obs else 0

print(f"  Insured-years observed          : {len(eff_obs):,}")
print(f"  Insured-years with any GUL      : {len(eff_nonzero):,}  ({pct_loss_years:.1f}%)")
print()
print(f"  {'Metric':<30}  {'Effective':>12}  {'Raw demand':>12}")
print(f"  {'-'*30}  {'-'*12}  {'-'*12}")
if eff_nonzero:
    metrics = [
        ("Mean (all years)",   statistics.mean(eff_obs),             statistics.mean(raw_obs)),
        ("Mean (loss years)",  statistics.mean(eff_nonzero),         statistics.mean(raw_nonzero)),
        ("Median (loss years)",statistics.median(eff_nonzero),       statistics.median(raw_nonzero)),
        ("P90 (loss years)",   percentile(eff_nonzero, 90),          percentile(raw_nonzero, 90)),
        ("P99 (loss years)",   percentile(eff_nonzero, 99),          percentile(raw_nonzero, 99)),
        ("Max (loss years)",   max(eff_nonzero),                     max(raw_nonzero)),
    ]
    for label, eff, raw in metrics:
        print(f"  {label:<30}  {fmt_gbp(eff):>12}  {fmt_gbp(raw):>12}")
print()

print("  Annual GUL as % of sum_insured (loss years, effective):")
gul_as_pct_si = []
for y in all_years:
    for iid in all_insured_ids:
        gul = gul_by_year_insured[y].get(iid, 0)
        si  = insured_si.get(iid)
        if gul > 0 and si and si > 0:
            gul_as_pct_si.append(100 * gul / si)
if gul_as_pct_si:
    print(f"    Mean   : {statistics.mean(gul_as_pct_si):.2f}%")
    print(f"    Median : {statistics.median(gul_as_pct_si):.2f}%")
    print(f"    P90    : {percentile(gul_as_pct_si, 90):.2f}%")
    print(f"    P99    : {percentile(gul_as_pct_si, 99):.2f}%")
    print(f"    Max    : {max(gul_as_pct_si):.2f}%")
print()

# ── Section 6: Per-territory insured-level summary ────────────────────────────

print("=" * 70)
print("SECTION 6 — PER-TERRITORY INSURED-LEVEL SUMMARY")
print("  (mean annual GUL per insured, and as % of their sum_insured)")
print("=" * 70)

territory_insureds = collections.defaultdict(set)
for iid, t in insured_territory.items():
    territory_insureds[t].add(iid)

print(f"  {'Territory':>8}  {'#Insureds':>9}  {'Mean GUL/yr':>12}  {'as %SI':>8}  {'P90 GUL/yr':>12}  {'SumInsured':>12}")
print(f"  {'-'*8}  {'-'*9}  {'-'*12}  {'-'*8}  {'-'*12}  {'-'*12}")

for territory in sorted(territory_insureds):
    iids = territory_insureds[territory]
    si_sample = [insured_si[i] for i in iids if i in insured_si]
    mean_si = statistics.mean(si_sample) if si_sample else 0

    # Annual GUL per insured, averaged across all years
    per_insured_annual_gul = []
    for iid in iids:
        yearly = [gul_by_year_insured[y].get(iid, 0) for y in all_years]
        per_insured_annual_gul.append(statistics.mean(yearly))

    mean_gul = statistics.mean(per_insured_annual_gul) if per_insured_annual_gul else 0
    p90_gul  = percentile(per_insured_annual_gul, 90) if per_insured_annual_gul else 0
    pct_si   = f"{100*mean_gul/mean_si:.2f}%" if mean_si > 0 else " n/a"

    print(f"  {territory:>8}  {len(iids):>9}  {fmt_gbp(mean_gul):>12}  {pct_si:>8}  {fmt_gbp(p90_gul):>12}  {fmt_gbp(mean_si):>12}")
print()

# ── Section 7: Realized damage fraction distribution by peril ─────────────────

print("=" * 70)
print("SECTION 7 — REALIZED DAMAGE FRACTIONS BY PERIL")
print("  (ground_up_loss / sum_insured, per-occurrence)")
print("=" * 70)
print(f"  {'Peril':>20}  {'N':>6}  {'Mean df':>8}  {'Median':>8}  {'P90':>8}  {'P99':>8}  {'Max':>8}")
print(f"  {'-'*20}  {'-'*6}  {'-'*8}  {'-'*8}  {'-'*8}  {'-'*8}  {'-'*8}")
for peril, dfs in sorted(damage_fractions_by_peril.items()):
    print(f"  {peril:>20}  {len(dfs):>6}  "
          f"{statistics.mean(dfs):>7.3f}  "
          f"{statistics.median(dfs):>7.3f}  "
          f"{percentile(dfs, 90):>7.3f}  "
          f"{percentile(dfs, 99):>7.3f}  "
          f"{max(dfs):>7.3f}  ")
print()

# ── Section 8: Cat-year vs quiet-year GUL split ───────────────────────────────

print("=" * 70)
print("SECTION 8 — CAT-YEAR vs QUIET-YEAR AGGREGATE GUL")
print("=" * 70)

quiet_years = [y for y in all_years if y not in cat_years]

def year_total_gul(y):
    return sum(gul_by_year_insured[y].values())

cat_totals   = [year_total_gul(y) for y in cat_years]
quiet_totals = [year_total_gul(y) for y in quiet_years]

def summarise_years(label, totals):
    if not totals:
        print(f"  {label}: no years")
        return
    print(f"  {label} ({len(totals)} year(s)):")
    print(f"    Mean annual GUL : {fmt_gbp(statistics.mean(totals))}")
    if len(totals) > 1:
        print(f"    Stdev           : {fmt_gbp(statistics.stdev(totals))}")
    print(f"    Min / Max       : {fmt_gbp(min(totals))} / {fmt_gbp(max(totals))}")

summarise_years("Cat years  ", cat_totals)
print()
summarise_years("Quiet years", quiet_totals)
print()
if cat_totals and quiet_totals:
    ratio = statistics.mean(cat_totals) / statistics.mean(quiet_totals)
    print(f"  Cat/quiet mean GUL ratio: {ratio:.2f}×")
print()

# ── Section 9: Cat event log ──────────────────────────────────────────────────

print("=" * 70)
print("SECTION 9 — CAT EVENT LOG")
print("=" * 70)
if not cat_events_by_year:
    print("  No LossEvents in stream.")
else:
    for y in all_years:
        evs = cat_events_by_year.get(y, [])
        flag = " <-- cat year" if y in cat_years else ""
        print(f"  Year {y}: {len(evs)} LossEvent(s){flag}")
        for (peril,) in evs:
            print(f"    - {peril}")
print()
