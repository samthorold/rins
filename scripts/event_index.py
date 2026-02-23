#!/usr/bin/env python3
"""
event_index.py — single-pass event index for rins analysis scripts.

Reads events.ndjson once and returns an EventIndex with every lookup table
needed by the analysis and verify scripts. Import build_index() and call it
once per script invocation; downstream scripts pay zero I/O cost for data
they already need.

Usage:
    from event_index import build_index, year, etype, loss_type
    idx = build_index()                          # reads "events.ndjson"
    idx = build_index("path/to/events.ndjson")   # optional path override
"""
import json
from dataclasses import dataclass
from pathlib import Path


def year(day: int) -> int:
    return day // 360 + 1


def etype(e: dict) -> str:
    ev = e["event"]
    return next(iter(ev)) if isinstance(ev, dict) else ev


def loss_type(peril: str) -> str:
    return "Attritional" if peril == "Attritional" else "Cat"


@dataclass
class EventIndex:
    # Full event list (for scripts that need to re-iterate in memory)
    events: list

    # Simulation metadata
    warmup_years: int
    analysis_years: object  # int | None
    max_day: int

    # Quoting chain — keyed by submission_id
    sub_sum_insured: dict   # submission_id -> sum_insured (from LeadQuoteRequested)
    sub_insured_id: dict    # submission_id -> insured_id (from LeadQuoteRequested)
    sub_request_day: dict   # submission_id -> day (from LeadQuoteRequested)
    sub_insurer: dict       # submission_id -> insurer_id (from LeadQuoteIssued)
    sub_atp: dict           # submission_id -> atp (from LeadQuoteIssued)
    sub_cat_exposure: dict  # submission_id -> cat_exposure_at_quote (from LeadQuoteIssued)
    sub_issued_day: dict    # submission_id -> day (from LeadQuoteIssued)
    sub_presented_day: dict # submission_id -> day (from QuotePresented)
    sub_premium: dict       # submission_id -> premium (from QuoteAccepted)
    sub_accepted_day: dict  # submission_id -> day (from QuoteAccepted)
    sub_policy: dict        # submission_id -> policy_id (from PolicyBound)
    sub_bound_day: dict     # submission_id -> day (from PolicyBound)

    # Quoting flow — keyed by (submission_id, insurer_id)
    quote_requested: dict   # (sub_id, ins_id) -> day (from LeadQuoteRequested)
    sub_responses: dict     # (sub_id, ins_id) -> list[str] (from LeadQuoteIssued/Declined/QuoteRejected)

    # Policy lifecycle — keyed by policy_id
    policy_insurer: dict    # policy_id -> insurer_id (from PolicyBound)
    policy_bound_day: dict  # policy_id -> day (from PolicyBound)
    policy_expiry_day: dict # policy_id -> day (from PolicyExpired)
    policy_sum_insured: dict  # policy_id -> sum_insured (derived: PolicyBound + sub_sum_insured)
    policy_sub: dict        # policy_id -> submission_id (from PolicyBound)

    # Insured-level — keyed by insured_id (populated from first LeadQuoteRequested per insured)
    insured_territory: dict # insured_id -> territory
    insured_si: dict        # insured_id -> sum_insured

    # Loss chain (raw lists; each item is a flat dict with 'day' merged in)
    loss_events: list       # [{day, peril, ...}, ...]
    insured_losses: list    # [{day, policy_id, insured_id, peril, ground_up_loss}, ...]
    claim_settled: list     # [{day, policy_id, insurer_id, peril, amount}, ...]

    # Derived
    cat_years: set          # years with ≥1 non-Attritional LossEvent


def build_index(path: str = "events.ndjson") -> EventIndex:
    """Read events.ndjson once and return a fully-populated EventIndex."""
    events = [
        json.loads(line)
        for line in Path(path).read_text().splitlines()
        if line.strip()
    ]

    warmup_years = 0
    analysis_years = None
    max_day = 0

    sub_sum_insured = {}
    sub_insured_id = {}
    sub_request_day = {}
    sub_insurer = {}
    sub_atp = {}
    sub_cat_exposure = {}
    sub_issued_day = {}
    sub_presented_day = {}
    sub_premium = {}
    sub_accepted_day = {}
    sub_policy = {}
    sub_bound_day = {}

    quote_requested = {}
    sub_responses = {}

    policy_insurer = {}
    policy_bound_day = {}
    policy_expiry_day = {}
    policy_sum_insured = {}
    policy_sub = {}

    insured_territory = {}
    insured_si = {}

    loss_events_list = []
    insured_losses_list = []
    claim_settled_list = []

    cat_years = set()

    for e in events:
        ev = e["event"]
        day = e["day"]
        if day > max_day:
            max_day = day

        if not isinstance(ev, dict):
            continue

        k = next(iter(ev))
        v = ev[k]

        if k == "SimulationStart":
            warmup_years = v.get("warmup_years", 0)
            analysis_years = v.get("analysis_years")

        elif k == "LeadQuoteRequested":
            sid = v["submission_id"]
            ins_id = v["insurer_id"]
            sub_sum_insured[sid] = v["risk"]["sum_insured"]
            sub_insured_id[sid] = v["insured_id"]
            sub_request_day[sid] = day
            quote_requested[(sid, ins_id)] = day
            # Capture insured metadata from first request per insured
            iid_insured = v["insured_id"]
            if iid_insured not in insured_territory:
                risk = v["risk"]
                insured_territory[iid_insured] = risk.get("territory", "Unknown")
                insured_si[iid_insured] = risk["sum_insured"]

        elif k == "LeadQuoteIssued":
            sid = v["submission_id"]
            ins_id = v["insurer_id"]
            sub_insurer[sid] = ins_id
            sub_atp[sid] = v["atp"]
            sub_cat_exposure[sid] = v.get("cat_exposure_at_quote", 0)
            sub_issued_day[sid] = day
            sub_responses.setdefault((sid, ins_id), []).append(k)

        elif k == "LeadQuoteDeclined":
            sid = v["submission_id"]
            ins_id = v["insurer_id"]
            sub_responses.setdefault((sid, ins_id), []).append(k)

        elif k == "QuotePresented":
            sub_presented_day[v["submission_id"]] = day

        elif k == "QuoteAccepted":
            sid = v["submission_id"]
            sub_premium[sid] = v["premium"]
            sub_accepted_day[sid] = day

        elif k == "QuoteRejected":
            sid = v["submission_id"]
            ins_id = v.get("insurer_id", "")
            sub_responses.setdefault((sid, ins_id), []).append(k)

        elif k == "PolicyBound":
            sid = v["submission_id"]
            pid = v["policy_id"]
            ins_id = v["insurer_id"]
            sub_policy[sid] = pid
            sub_bound_day[sid] = day
            policy_insurer[pid] = ins_id
            policy_bound_day[pid] = day
            policy_sub[pid] = sid
            si = sub_sum_insured.get(sid)
            if si is not None:
                policy_sum_insured[pid] = si

        elif k == "PolicyExpired":
            policy_expiry_day[v["policy_id"]] = day

        elif k == "LossEvent":
            record = dict(v)
            record["day"] = day
            loss_events_list.append(record)
            peril = v.get("peril", "")
            if peril != "Attritional":
                cat_years.add(year(day))

        elif k == "InsuredLoss":
            record = dict(v)
            record["day"] = day
            insured_losses_list.append(record)

        elif k == "ClaimSettled":
            record = dict(v)
            record["day"] = day
            claim_settled_list.append(record)

    return EventIndex(
        events=events,
        warmup_years=warmup_years,
        analysis_years=analysis_years,
        max_day=max_day,
        sub_sum_insured=sub_sum_insured,
        sub_insured_id=sub_insured_id,
        sub_request_day=sub_request_day,
        sub_insurer=sub_insurer,
        sub_atp=sub_atp,
        sub_cat_exposure=sub_cat_exposure,
        sub_issued_day=sub_issued_day,
        sub_presented_day=sub_presented_day,
        sub_premium=sub_premium,
        sub_accepted_day=sub_accepted_day,
        sub_policy=sub_policy,
        sub_bound_day=sub_bound_day,
        quote_requested=quote_requested,
        sub_responses=sub_responses,
        policy_insurer=policy_insurer,
        policy_bound_day=policy_bound_day,
        policy_expiry_day=policy_expiry_day,
        policy_sum_insured=policy_sum_insured,
        policy_sub=policy_sub,
        insured_territory=insured_territory,
        insured_si=insured_si,
        loss_events=loss_events_list,
        insured_losses=insured_losses_list,
        claim_settled=claim_settled_list,
        cat_years=cat_years,
    )
