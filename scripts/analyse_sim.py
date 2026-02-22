#!/usr/bin/env python3
"""
analyse_sim.py — structured year-over-year analysis of rins events.ndjson.

Run from the project root after `cargo run`:
    python3 scripts/analyse_sim.py
"""
import json, collections
from pathlib import Path

events = [json.loads(l) for l in Path("events.ndjson").read_text().splitlines() if l.strip()]

def year(day): return day // 360 + 1
def etype(e): return next(iter(e['event'])) if isinstance(e['event'], dict) else e['event']
def loss_type(peril): return 'Attritional' if peril == 'Attritional' else 'Cat'

# Read warm-up and analysis period from the SimulationStart event.
# Warm-up years are excluded from all output tables; they exist only to let the
# EWMA stabilise past the staggered year-1 partial-exposure artefact.
warmup_years = 0
analysis_years = None
for e in events:
    if isinstance(e['event'], dict) and 'SimulationStart' in e['event']:
        ss = e['event']['SimulationStart']
        warmup_years = ss.get('warmup_years', 0)
        analysis_years = ss.get('analysis_years')
        break

# --- event type counts ---
type_counts = collections.Counter(etype(e) for e in events)

# --- per-year buckets ---
submissions   = collections.Counter()  # CoverageRequested
policies      = collections.Counter()  # PolicyBound
declines      = collections.Counter()  # QuoteRejected (future)
quote_req     = collections.Counter()  # LeadQuoteRequested
quote_iss     = collections.Counter()  # LeadQuoteIssued
quote_pres    = collections.Counter()  # QuotePresented
quote_acc     = collections.Counter()  # QuoteAccepted
loss_events   = collections.Counter()  # year -> count
insured_losses= collections.defaultdict(lambda: collections.defaultdict(int))  # year -> peril -> GUL
insured_gul   = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> insured_id -> GUL
insured_gul_split = collections.defaultdict(  # year -> insured_id -> type -> GUL
    lambda: collections.defaultdict(lambda: collections.defaultdict(int)))
gul_split     = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> type -> GUL
claims        = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> insurer_id -> sum
claims_split  = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> type -> amount

# Per-insurer premiums (from QuoteAccepted, linked via submission_id → PolicyBound insurer)
# submission_id → premium (from QuoteAccepted)
sub_premium   = {}
# submission_id → insurer_id (from PolicyBound)
sub_insurer   = {}
premiums      = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> insurer_id -> sum

# ATP / exposure tracking (two-channel pricing)
sub_atp              = {}   # submission_id -> atp (from LeadQuoteIssued)
sub_cat_exposure     = {}   # submission_id -> cat_exposure_at_quote (from LeadQuoteIssued)
sub_sum_insured      = {}   # submission_id -> sum_insured (from LeadQuoteRequested)
atp_per_insurer      = collections.defaultdict(lambda: collections.defaultdict(int))
# year -> insurer_id -> cumulative ATP (sum over bound policies)
total_exposure       = collections.defaultdict(int)  # year -> total bound sum_insured
# Cat exposure written per insurer per year (sum of sum_insured from PolicyBound for cat risks)
cat_exposure_written = collections.defaultdict(lambda: collections.defaultdict(int))
# year -> insurer_id -> sum of cat_exposure_at_quote at time of quoting
cat_exposure_at_quote_sum = collections.defaultdict(lambda: collections.defaultdict(int))

# Policy count per insurer per year
insurer_policy_count = collections.defaultdict(lambda: collections.defaultdict(int))

# Policy metadata: policy_id -> {insured_id, insurer_id, submission_id}
policy_meta   = {}

for e in events:
    d, ev = e['day'], e['event']
    if not isinstance(ev, dict): continue
    y = year(d)
    k = next(iter(ev))
    v = ev[k]

    if k == 'CoverageRequested':
        submissions[y] += 1
    elif k == 'LeadQuoteRequested':
        quote_req[y] += 1
        sub_sum_insured[v['submission_id']] = v['risk']['sum_insured']
    elif k == 'LeadQuoteIssued':
        quote_iss[y] += 1
        sub_atp[v['submission_id']] = v['atp']
        sub_cat_exposure[v['submission_id']] = v.get('cat_exposure_at_quote', 0)
    elif k == 'QuotePresented':
        quote_pres[y] += 1
    elif k == 'QuoteAccepted':
        quote_acc[y] += 1
        sub_premium[v['submission_id']] = v['premium']
    elif k == 'QuoteRejected':
        declines[y] += 1
    elif k == 'PolicyBound':
        policies[y] += 1
        sid  = v['submission_id']
        iid  = v['insurer_id']
        pid  = v['policy_id']
        sub_insurer[sid] = iid
        insurer_policy_count[y][iid] += 1
        prem = sub_premium.get(sid, 0)
        premiums[y][iid] += prem
        atp_per_insurer[y][iid] += sub_atp.get(sid, 0)
        total_exposure[y]        += sub_sum_insured.get(sid, 0)
        policy_meta[pid] = {'insurer_id': iid, 'submission_id': sid}
        # cat_exposure_at_quote from the matching LeadQuoteIssued
        cat_exposure_at_quote_sum[y][iid] += sub_cat_exposure.get(sid, 0)
        # Written cat exposure: use sum_insured from PolicyBound if present (new field),
        # otherwise fall back to sub_sum_insured from LeadQuoteRequested.
        bound_si = v.get('sum_insured', sub_sum_insured.get(sid, 0))
        cat_exposure_written[y][iid] += bound_si
    elif k == 'LossEvent':
        loss_events[y] += 1
    elif k == 'InsuredLoss':
        insured_losses[y][v['peril']] += v['ground_up_loss']
        insured_gul[y][v['insured_id']] += v['ground_up_loss']
        lt = loss_type(v['peril'])
        gul_split[y][lt] += v['ground_up_loss']
        insured_gul_split[y][v['insured_id']][lt] += v['ground_up_loss']
    elif k == 'ClaimSettled':
        iid = v['insurer_id']
        claims[y][iid] += v['amount']
        ct = loss_type(v['peril'])
        claims_split[y][ct] += v['amount']

years = sorted(y for y in (set(submissions) | set(policies)) if y > warmup_years)
all_insurers = sorted({i for yy in premiums.values() for i in yy} |
                      {i for yy in claims.values() for i in yy})

if warmup_years:
    print(f"=== Warm-up: {warmup_years} year(s) excluded from all output tables ===")
    print(f"    (analysis period: years {warmup_years+1}–{warmup_years + (analysis_years or '?')})")

print("=== Event type counts ===")
for t, n in type_counts.most_common(): print(f"  {t}: {n}")

print("\n=== Year-over-year summary ===")
print(f"{'Year':>4}  {'CovReq':>6}  {'Bound':>5}  {'Bind%':>6}  {'LQReq':>5}  {'LQIss':>5}  {'QPres':>5}  {'QAcc':>5}  {'QRej':>5}  {'LossEv':>6}")
for y in years:
    s, p = submissions[y], policies[y]
    qr, qi, qp, qa, qd = quote_req[y], quote_iss[y], quote_pres[y], quote_acc[y], declines[y]
    bind_pct = f"{100*p/s:.1f}" if s else "-"
    print(f"  {y:>2}    {s:>6}  {p:>5}  {bind_pct:>6}  {qr:>5}  {qi:>5}  {qp:>5}  {qa:>5}  {qd:>5}  {loss_events[y]:>6}")

print("\n=== Premiums by insurer per year (cents) ===")
hdr = f"{'Year':>4}" + "".join(f"  Ins{i:>2}" for i in all_insurers)
print(hdr)
for y in years:
    row = f"  {y:>2}" + "".join(f"  {premiums[y].get(i,0):>8}" for i in all_insurers)
    print(row)

print("\n=== Policies written per insurer per year ===")
print(hdr)
for y in years:
    row = f"  {y:>2}" + "".join(f"  {insurer_policy_count[y].get(i,0):>8}" for i in all_insurers)
    print(row)

print("\n=== Loss events per year ===")
for y in years:
    print(f"  Year {y}: {loss_events[y]} LossEvent(s)")

print("\n=== Ground-up insured loss by peril per year (cents) ===")
all_il_perils = sorted({p for yy in insured_losses.values() for p in yy})
for p in all_il_perils:
    row = f"  {p:<22}" + "".join(f"  Y{y}:{insured_losses[y].get(p,0):>16}" for y in years)
    print(row)

print("\n=== GUL split: Attritional vs Cat per year ===")
print(f"{'Year':>4}  {'AttrGUL':>16}  {'CatGUL':>16}  {'TotalGUL':>16}  {'Attr%':>6}  {'Cat%':>6}")
for y in years:
    ag = gul_split[y].get('Attritional', 0)
    cg = gul_split[y].get('Cat', 0)
    tg = ag + cg
    ap = f"{100*ag/tg:.1f}" if tg else "-"
    cp = f"{100*cg/tg:.1f}" if tg else "-"
    print(f"  {y:>2}    {ag:>16}  {cg:>16}  {tg:>16}  {ap:>6}  {cp:>6}")

print("\n=== Insured GUL summary per year ===")
print(f"{'Year':>4}  {'Insureds':>8}  {'TotalGUL':>16}  {'TopInsured':>10}  {'TopGUL':>16}  {'Top%':>6}  {'GUL-HHI':>8}")
for y in years:
    ig = insured_gul[y]
    if not ig:
        print(f"  {y:>2}         n/a")
        continue
    total_gul = sum(ig.values())
    top_id, top_gul = max(ig.items(), key=lambda x: x[1])
    top_pct = 100 * top_gul / total_gul if total_gul else 0
    hhi_gul = sum((v / total_gul * 100) ** 2 for v in ig.values()) if total_gul else 0
    print(f"  {y:>2}    {len(ig):>8}  {total_gul:>16}  {top_id:>10}  {top_gul:>16}  {top_pct:>6.1f}  {hhi_gul:>8.0f}")

print("\n=== Top 10 insureds by total GUL (all years) ===")
all_insured_gul: dict = collections.defaultdict(int)
for yy in insured_gul.values():
    for ins_id, gul in yy.items():
        all_insured_gul[ins_id] += gul
print(f"  {'InsuredId':>10}  {'TotalGUL':>16}  {'Share%':>7}")
total_all_gul = sum(all_insured_gul.values())
top_insureds = [i for i, _ in sorted(all_insured_gul.items(), key=lambda x: -x[1])[:10]]
for ins_id in top_insureds:
    gul = all_insured_gul[ins_id]
    share = 100 * gul / total_all_gul if total_all_gul else 0
    print(f"  {ins_id:>10}  {gul:>16}  {share:>7.1f}")

print("\n=== Per-insured GUL by year (top 10 by lifetime GUL) ===")
hdr_ins = f"  {'InsuredId':>10}" + "".join(f"  {'Y'+str(y):>16}" for y in years)
print(hdr_ins)
for ins_id in top_insureds:
    row = f"  {ins_id:>10}" + "".join(f"  {insured_gul[y].get(ins_id,0):>16}" for y in years)
    print(row)

print("\n=== Per-insured Attritional vs Cat GUL (top 10, all years) ===")
print(f"  {'InsuredId':>10}  {'AttrGUL':>16}  {'CatGUL':>16}  {'Attr%':>6}  {'Cat%':>6}")
for ins_id in top_insureds:
    ag = sum(insured_gul_split[y][ins_id].get('Attritional', 0) for y in years)
    cg = sum(insured_gul_split[y][ins_id].get('Cat', 0) for y in years)
    tg = ag + cg
    ap = f"{100*ag/tg:.1f}" if tg else "-"
    cp = f"{100*cg/tg:.1f}" if tg else "-"
    print(f"  {ins_id:>10}  {ag:>16}  {cg:>16}  {ap:>6}  {cp:>6}")

print("\n=== Claims settled by insurer per year (cents) ===")
hdr2 = f"{'Year':>4}" + "".join(f"  Ins{i:>2}" for i in all_insurers)
print(hdr2)
for y in years:
    row2 = f"  {y:>2}" + "".join(f"  {claims[y].get(i,0):>9}" for i in all_insurers)
    print(row2)

print("\n=== Claims split: Attritional vs Cat per year ===")
print(f"{'Year':>4}  {'AttrClaims':>16}  {'CatClaims':>16}  {'TotalClaims':>16}  {'Attr%':>6}  {'Cat%':>6}")
for y in years:
    ac = claims_split[y].get('Attritional', 0)
    cc = claims_split[y].get('Cat', 0)
    tc = ac + cc
    ap = f"{100*ac/tc:.1f}" if tc else "-"
    cp = f"{100*cc/tc:.1f}" if tc else "-"
    print(f"  {y:>2}    {ac:>16}  {cc:>16}  {tc:>16}  {ap:>6}  {cp:>6}")

print("\n=== Per-insurer loss ratio per year ===")
hdr3 = f"{'Year':>4}" + "".join(f"  Ins{i:>2} LR%" for i in all_insurers)
print(hdr3)
flagged = []
for y in years:
    row3 = f"  {y:>2}"
    for i in all_insurers:
        prem = premiums[y].get(i, 0)
        clm  = claims[y].get(i, 0)
        if prem > 0:
            lr_pct = 100 * clm / prem
            cell = f"{lr_pct:>8.1f}"
            if lr_pct > 100:
                flagged.append(f"  WARN year={y} insurer={i}: LR={lr_pct:.1f}% (claims={clm} premium={prem})")
        else:
            cell = f"{'n/a':>8}"
        row3 += f"  {cell}"
    print(row3)
if flagged:
    print()
    for w in flagged: print(w)

print("\n=== Market-level Attritional vs Cat loss ratio per year ===")
print(f"{'Year':>4}  {'TotalPrem':>16}  {'AttrClaims':>14}  {'AttrLR%':>8}  {'CatClaims':>14}  {'CatLR%':>8}")
for y in years:
    total_prem = sum(premiums[y].values())
    ac = claims_split[y].get('Attritional', 0)
    cc = claims_split[y].get('Cat', 0)
    alr = f"{100*ac/total_prem:.1f}" if total_prem else "-"
    clr = f"{100*cc/total_prem:.1f}" if total_prem else "-"
    print(f"  {y:>2}    {total_prem:>16}  {ac:>14}  {alr:>8}  {cc:>14}  {clr:>8}")

print("\n=== Market HHI per year (by insurer bound premium) ===")
print(f"{'Year':>4}  {'HHI':>6}  (0=perfect competition, 10000=monopoly)")
for y in years:
    total = sum(premiums[y].values())
    if total:
        hhi = sum((v / total * 100) ** 2 for v in premiums[y].values())
        print(f"  {y:>2}    {hhi:>6.0f}")
    else:
        print(f"  {y:>2}      n/a")

print("\n=== Market rate on line per year ===")
print(f"{'Year':>4}  {'TotalPrem':>16}  {'TotalExposure':>16}  {'RateOnLine%':>12}")
for y in years:
    tp  = sum(premiums[y].values())
    exp = total_exposure[y]
    rol = f"{100 * tp / exp:.1f}%" if exp else "-"
    print(f"  {y:>2}    {tp:>16}  {exp:>16}  {rol:>12}")

print("\n=== ATP adequacy ratio per year ===")
print(f"{'Year':>4}  {'TotalATP':>16}  {'TotalClaims':>16}  {'Adequacy':>9}  Note")
for y in years:
    total_atp    = sum(atp_per_insurer[y].values())
    total_claims = sum(claims[y].values())
    if total_atp:
        adequacy = total_claims / total_atp
        note = "*** ABOVE FLOOR ***" if adequacy > 1.0 else "(expect ~0.70 in benign year)"
        print(f"  {y:>2}    {total_atp:>16}  {total_claims:>16}  {adequacy:>9.2f}  {note}")
    else:
        print(f"  {y:>2}    {'n/a':>16}  {total_claims:>16}  {'-':>9}")

print("\n=== Underwriter margin per insurer per year ===")
print("(premium - atp) / atp — positive = premium above ATP floor")
hdr_um = f"{'Year':>4}" + "".join(f"  Ins{i:>2}" for i in all_insurers)
print(hdr_um)
margin_warns = []
for y in years:
    row_um = f"  {y:>2}"
    for i in all_insurers:
        atp_i  = atp_per_insurer[y].get(i, 0)
        prem_i = premiums[y].get(i, 0)
        if atp_i:
            margin = (prem_i - atp_i) / atp_i * 100
            cell = f"{margin:>7.1f}%"
            if margin < 0:
                margin_warns.append(f"  WARN year={y} insurer={i}: premium < atp (underwriting below actuarial floor)")
        else:
            cell = f"{'n/a':>8}"
        row_um += f"  {cell}"
    print(row_um)
if margin_warns:
    print()
    for w in margin_warns: print(w)

print("\n=== Cat exposure written per insurer per year (sum_insured, cents) ===")
print("(all bound policies, regardless of peril mix — use as written exposure proxy)")
hdr_cat = f"{'Year':>4}" + "".join(f"  Ins{i:>2}" for i in all_insurers) + f"  {'Total':>12}"
print(hdr_cat)
for y in years:
    total_cat = sum(cat_exposure_written[y].values())
    row_cat = f"  {y:>2}" + "".join(f"  {cat_exposure_written[y].get(i,0):>8}" for i in all_insurers) + f"  {total_cat:>12}"
    print(row_cat)

print("\n=== Cat exposure at time of quoting per insurer per year (cumulative, cents) ===")
print("(sum of cat_exposure_at_quote from LeadQuoteIssued for policies that bound)")
print(hdr_cat)
for y in years:
    total_caq = sum(cat_exposure_at_quote_sum[y].values())
    row_caq = f"  {y:>2}" + "".join(f"  {cat_exposure_at_quote_sum[y].get(i,0):>8}" for i in all_insurers) + f"  {total_caq:>12}"
    print(row_caq)
