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

# --- event type counts ---
type_counts = collections.Counter(etype(e) for e in events)

# --- per-year buckets ---
submissions   = collections.Counter()  # year -> count
policies      = collections.Counter()
declines      = collections.Counter()
quote_req     = collections.Counter()
premiums      = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> syn_id -> sum
losses        = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> (peril,region) -> sum
claims        = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> syn_id -> sum

# lead/follower quoting funnel
lead_req      = collections.Counter()   # year -> count
lead_iss      = collections.Counter()
lead_dec      = collections.Counter()
foll_req      = collections.Counter()
foll_iss      = collections.Counter()

# per-syndicate premiums from PolicyBound panel entries
bound_premiums = collections.defaultdict(lambda: collections.defaultdict(int))  # year -> syn_id -> sum

for e in events:
    d, ev = e['day'], e['event']
    if not isinstance(ev, dict): continue
    y = year(d)
    k = next(iter(ev))
    v = ev[k]
    if k == 'SubmissionArrived':  submissions[y] += 1
    elif k == 'PolicyBound':
        policies[y] += 1
        for entry in v['panel']['entries']:
            bound_premiums[y][entry['syndicate_id']] += entry['premium']
    elif k == 'QuoteDeclined':    declines[y] += 1
    elif k == 'QuoteRequested':
        quote_req[y] += 1
        if v.get('is_lead'):  lead_req[y] += 1
        else:                 foll_req[y] += 1
    elif k == 'QuoteIssued':
        if v.get('is_lead'):
            lead_iss[y] += 1
            premiums[y][v['syndicate_id']] += v['premium']
        else:
            foll_iss[y] += 1
    elif k == 'LossEvent':
        losses[y][(v['peril'], v['region'])] += v['severity']
    elif k == 'ClaimSettled':
        claims[y][v['syndicate_id']] += v['amount']

years = sorted(set(submissions) | set(policies))

print("=== Event type counts ===")
for t, n in type_counts.most_common(): print(f"  {t}: {n}")

print("\n=== Year-over-year summary ===")
print(f"{'Year':>4}  {'Subs':>5}  {'Bound':>5}  {'Bind%':>6}  {'QReq':>5}  {'QDec':>5}  {'Dec%':>6}")
for y in years:
    s, p, qr, qd = submissions[y], policies[y], quote_req[y], declines[y]
    bind_pct = f"{100*p/s:.1f}" if s else "-"
    dec_pct  = f"{100*qd/qr:.1f}" if qr else "-"
    print(f"  {y:>2}    {s:>5}  {p:>5}  {bind_pct:>6}  {qr:>5}  {qd:>5}  {dec_pct:>6}")

print("\n=== Lead premium by syndicate (pence) ===")
all_syns = sorted({s for yy in premiums.values() for s in yy})
hdr = f"{'Year':>4}" + "".join(f"  Syn{s:>2}" for s in all_syns)
print(hdr)
for y in years:
    row = f"  {y:>2}" + "".join(f"  {premiums[y].get(s,0):>6}" for s in all_syns)
    print(row)

print("\n=== Loss severity by (peril, territory) per year ===")
all_perils = sorted({pk for yy in losses.values() for pk in yy})
for pk in all_perils:
    row = f"  {pk[0]:<22} {pk[1]:<8}" + "".join(f"  Y{y}:{losses[y].get(pk,0):>9}" for y in years)
    print(row)

print("\n=== Claims settled by syndicate per year (pence) ===")
all_csyn = sorted({s for yy in claims.values() for s in yy})
hdr2 = f"{'Year':>4}" + "".join(f"  Syn{s:>2}" for s in all_csyn)
print(hdr2)
for y in years:
    row2 = f"  {y:>2}" + "".join(f"  {claims[y].get(s,0):>9}" for s in all_csyn)
    print(row2)

print("\n=== Lead vs follower quoting funnel ===")
print(f"{'Year':>4}  {'LdReq':>6}  {'LdIss':>6}  {'LdDec':>6}  {'LdConv%':>8}  |  {'FlReq':>6}  {'FlIss':>6}  {'FlDec':>6}  {'FlConv%':>8}")
for y in years:
    lr, li = lead_req[y], lead_iss[y]
    ld = lead_dec[y]  # QuoteDeclined has no is_lead; this will be 0 (see note below)
    fr, fi = foll_req[y], foll_iss[y]
    fd = fr - fi  # inferred: follower declines = follower requests - follower quotes issued
    lc = f"{100*li/lr:.1f}" if lr else "-"
    fc = f"{100*fi/fr:.1f}" if fr else "-"
    print(f"  {y:>2}    {lr:>6}  {li:>6}  {ld:>6}  {lc:>8}  |  {fr:>6}  {fi:>6}  {fd:>6}  {fc:>8}")
print("  Note: LdDec is always 0 (QuoteDeclined has no is_lead field); FlDec is inferred.")

print("\n=== Per-syndicate loss ratio (bound premium vs claims) ===")
all_lr_syns = sorted({s for yy in bound_premiums.values() for s in yy} | {s for yy in claims.values() for s in yy})
hdr3 = f"{'Year':>4}" + "".join(f"  Syn{s:>2} LR%" for s in all_lr_syns)
print(hdr3)
flagged = []
for y in years:
    row3 = f"  {y:>2}"
    for s in all_lr_syns:
        prem = bound_premiums[y].get(s, 0)
        clm  = claims[y].get(s, 0)
        if prem > 0:
            lr_pct = 100 * clm / prem
            cell = f"{lr_pct:>8.1f}"
            if lr_pct > 100:
                flagged.append(f"  WARN year={y} syndicate={s}: LR={lr_pct:.1f}% (claims={clm} premium={prem})")
        else:
            cell = f"{'n/a':>8}"
        row3 += f"  {cell}"
    print(row3)
if flagged:
    print()
    for w in flagged: print(w)
