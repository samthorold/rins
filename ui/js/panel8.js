// Panel 8: Invariant Dashboard.
//
// Reimplements `verify_mechanics` and `verify_integrity` from `src/analysis.rs`
// in JavaScript so the viewer can render structural-health badges from a loaded
// events.ndjson stream.
//
//   verifyMechanics(events) → [{ check, detail, ...}]
//   verifyIntegrity(events) → [{ check, detail, ...}]
//   prepPanel8Data(db, opts) → { mechanics: [...], integrity: [...] }
//   renderPanel8(data, opts) → DOM element (or string when asString: true)
//
// The 19 checks correspond to the issue spec (7 mechanics + 12 integrity).

export const MECHANICS_CHECKS = [
  ["DayOffsetChain",            "Day-offset chain"],
  ["LossBeforeBound",           "Loss before bound"],
  ["AttrNotStrictlyPostBound",  "Attritional strictly post-bound"],
  ["PolicyExpiredTiming",       "PolicyExpired timing"],
  ["ClaimAfterExpiry",          "Claim after expiry"],
  ["CatFractionInconsistent",   "Cat fraction consistency"],
  ["InvalidDamageFraction",     "Damage fraction valid"],
];

export const INTEGRITY_CHECKS = [
  ["GulExceedsSumInsured",            "GUL ≤ sum insured"],
  ["AggregateClaimExceedsSumInsured", "Aggregate claim ≤ sum insured / yr"],
  ["ClaimWithoutMatchingLoss",        "Claim has matching AssetDamage"],
  ["ClaimAmountZero",                 "Claim amount > 0"],
  ["ClaimInsurerMismatch",            "Claim insurer in panel"],
  ["QuoteAcceptedWithoutPolicyBound", "Accepted quote → PolicyBound"],
  ["PolicyBoundInsurerMismatch",      "PolicyBound leader matches"],
  ["DuplicatePolicyBound",            "No duplicate PolicyBound"],
  ["PolicyExpiredWithoutBound",       "PolicyExpired references bound"],
  ["LeadQuoteOrphanRequest",          "LeadQuoteRequested → response"],
  ["LeadQuoteDuplicateResponse",      "≤ 1 lead response per (sub, ins)"],
  ["LeadQuoteOrphanResponse",         "Lead response → prior request"],
];

// ── Verifiers ──────────────────────────────────────────────────────────────

export function verifyMechanics(events) {
  const violations = [];
  const lqrDay = new Map();      // submission_id → first LQR day
  const qaDay = new Map();       // submission_id → QuoteAccepted day
  const policyFromSub = new Map(); // submission_id → policy_id
  const expiryDay = new Map();   // policy_id → expiry day
  const insuredCrDay = new Map(); // insured_id → first CR day
  const insuredSi = new Map();   // insured_id → sum_insured

  for (const e of events) {
    const day = e.day;
    const d = e.data;
    switch (e.type) {
      case "CoverageRequested":
        if (!insuredCrDay.has(d.insured_id)) insuredCrDay.set(d.insured_id, day);
        if (!insuredSi.has(d.insured_id)) insuredSi.set(d.insured_id, d.risk?.sum_insured ?? 0);
        break;
      case "LeadQuoteRequested":
        if (!lqrDay.has(d.submission_id)) lqrDay.set(d.submission_id, day);
        break;
      case "QuoteAccepted":
        qaDay.set(d.submission_id, day);
        break;
      case "PolicyBound": {
        policyFromSub.set(d.submission_id, d.policy_id);
        const lqr = lqrDay.get(d.submission_id);
        if (lqr !== undefined) {
          const expected = lqr + 2;
          if (day !== expected) {
            violations.push({
              check: "DayOffsetChain",
              submission_id: d.submission_id,
              detail: `PolicyBound at day ${day}, expected ${expected} (LeadQuoteRequested at ${lqr})`,
            });
          }
        }
        break;
      }
      case "PolicyExpired":
        expiryDay.set(d.policy_id, day);
        break;
    }
  }

  for (const [subId, pid] of policyFromSub) {
    const qa = qaDay.get(subId);
    const actual = expiryDay.get(pid);
    if (qa !== undefined && actual !== undefined) {
      const expected = qa + 361;
      if (actual !== expected) {
        violations.push({
          check: "PolicyExpiredTiming",
          policy_id: pid,
          expected,
          actual,
          detail: `policy ${pid}: expected expiry day ${expected}, got ${actual}`,
        });
      }
    }
  }

  for (const e of events) {
    const day = e.day;
    const d = e.data;
    if (e.type === "AssetDamage") {
      const crDay = insuredCrDay.get(d.insured_id);
      if (crDay !== undefined) {
        if (day < crDay) {
          violations.push({
            check: "LossBeforeBound",
            insured_id: d.insured_id,
            loss_day: day,
            bound_day: crDay,
            detail: `insured ${d.insured_id}: loss day ${day} < first CR day ${crDay}`,
          });
        }
        if (d.peril === "Attritional" && day <= crDay) {
          violations.push({
            check: "AttrNotStrictlyPostBound",
            insured_id: d.insured_id,
            loss_day: day,
            bound_day: crDay,
            detail: `insured ${d.insured_id}: attritional loss day ${day} ≤ CR day ${crDay}`,
          });
        }
      }
      if (d.peril === "WindstormAtlantic") {
        const si = insuredSi.get(d.insured_id);
        if (si !== undefined && d.ground_up_loss > si) {
          violations.push({
            check: "CatFractionInconsistent",
            insured_id: d.insured_id,
            day,
            detail: `insured ${d.insured_id} gul ${d.ground_up_loss} > sum_insured ${si}`,
          });
        }
      }
    } else if (e.type === "ClaimSettled") {
      const exp = expiryDay.get(d.policy_id);
      if (exp !== undefined && day > exp) {
        violations.push({
          check: "ClaimAfterExpiry",
          policy_id: d.policy_id,
          claim_day: day,
          expiry_day: exp,
          detail: `policy ${d.policy_id}: claim day ${day} > expiry day ${exp}`,
        });
      }
    } else if (e.type === "LossEvent") {
      const df = d.damage_fraction;
      if (typeof df === "number" && (df <= 0 || df > 1)) {
        violations.push({
          check: "InvalidDamageFraction",
          event_id: d.event_id,
          damage_fraction: df,
          detail: `event ${d.event_id} damage_fraction ${df} not in (0, 1]`,
        });
      }
    }
  }

  return violations;
}

export function verifyIntegrity(events) {
  let maxDay = 0;
  const policySi = new Map();
  const policyLeader = new Map();
  const policyPanel = new Map();    // policy_id → Set(insurer_id)
  const policyInsured = new Map();
  const insuredSi = new Map();
  const subLeaderQuoted = new Map(); // submission_id → leader_id (from QA)
  const subAcceptedDay = new Map();
  const subPolicy = new Map();
  const policyBindCount = new Map();
  const boundPolicies = new Set();
  const lossKeys = new Set();        // `${day}|${insured_id}`
  const claimAgg = new Map();        // `${policy_id}|${year}` → sum
  const claimSettledList = [];       // [{day, policy_id, insurer_id, amount}]
  const leadRequested = new Map();   // `${sub}|${ins}` → day
  const leadResponses = new Map();   // `${sub}|${ins}` → count
  const orphanResponses = [];

  for (const e of events) {
    const day = e.day;
    if (day > maxDay) maxDay = day;
    const d = e.data;
    switch (e.type) {
      case "CoverageRequested":
        if (!insuredSi.has(d.insured_id)) insuredSi.set(d.insured_id, d.risk?.sum_insured ?? 0);
        break;
      case "QuoteAccepted":
        subAcceptedDay.set(d.submission_id, day);
        subLeaderQuoted.set(d.submission_id, d.leader_id);
        break;
      case "PolicyBound": {
        policySi.set(d.policy_id, d.sum_insured);
        const panel = d.panel ?? [];
        if (panel.length > 0) policyLeader.set(d.policy_id, panel[0][0]);
        policyPanel.set(d.policy_id, new Set(panel.map(([id]) => id)));
        policyInsured.set(d.policy_id, d.insured_id);
        subPolicy.set(d.submission_id, d.policy_id);
        policyBindCount.set(d.policy_id, (policyBindCount.get(d.policy_id) ?? 0) + 1);
        boundPolicies.add(d.policy_id);
        break;
      }
      case "AssetDamage":
        lossKeys.add(`${day}|${d.insured_id}`);
        break;
      case "ClaimSettled": {
        const key = `${d.policy_id}|${e.year}`;
        claimAgg.set(key, (claimAgg.get(key) ?? 0) + (d.amount ?? 0));
        claimSettledList.push({ day, policy_id: d.policy_id, insurer_id: d.insurer_id, amount: d.amount ?? 0 });
        break;
      }
      case "LeadQuoteRequested": {
        const key = `${d.submission_id}|${d.insurer_id}`;
        if (!leadRequested.has(key)) leadRequested.set(key, day);
        break;
      }
      case "LeadQuoteIssued":
      case "LeadQuoteDeclined": {
        const key = `${d.submission_id}|${d.insurer_id}`;
        if (!leadRequested.has(key)) {
          orphanResponses.push({ submission_id: d.submission_id, insurer_id: d.insurer_id, day, kind: e.type });
        }
        leadResponses.set(key, (leadResponses.get(key) ?? 0) + 1);
        break;
      }
    }
  }

  const violations = [];

  // Check 1
  for (const e of events) {
    if (e.type !== "AssetDamage") continue;
    const si = insuredSi.get(e.data.insured_id);
    if (si !== undefined && e.data.ground_up_loss > si) {
      violations.push({
        check: "GulExceedsSumInsured",
        insured_id: e.data.insured_id,
        day: e.day,
        peril: e.data.peril,
        gul: e.data.ground_up_loss,
        sum_insured: si,
        detail: `insured ${e.data.insured_id} day ${e.day}: gul ${e.data.ground_up_loss} > sum_insured ${si}`,
      });
    }
  }

  // Check 2
  for (const [key, agg] of claimAgg) {
    const [pidStr, yearStr] = key.split("|");
    const pid = Number(pidStr);
    const si = policySi.get(pid);
    if (si !== undefined && agg > si) {
      violations.push({
        check: "AggregateClaimExceedsSumInsured",
        policy_id: pid,
        year: Number(yearStr),
        aggregate: agg,
        sum_insured: si,
        detail: `policy ${pid} year ${yearStr}: aggregate ${agg} > sum_insured ${si}`,
      });
    }
  }

  // Checks 3, 4, 5
  for (const c of claimSettledList) {
    const insuredId = policyInsured.get(c.policy_id);
    const hasMatch = insuredId !== undefined && lossKeys.has(`${c.day}|${insuredId}`);
    if (!hasMatch) {
      violations.push({
        check: "ClaimWithoutMatchingLoss",
        policy_id: c.policy_id,
        day: c.day,
        detail: `policy ${c.policy_id} day ${c.day}: no matching AssetDamage`,
      });
    }
    if (c.amount === 0) {
      violations.push({
        check: "ClaimAmountZero",
        policy_id: c.policy_id,
        day: c.day,
        detail: `policy ${c.policy_id} day ${c.day}: claim amount is 0`,
      });
    }
    const panel = policyPanel.get(c.policy_id);
    if (panel && !panel.has(c.insurer_id)) {
      const bound = policyLeader.get(c.policy_id) ?? 0;
      violations.push({
        check: "ClaimInsurerMismatch",
        policy_id: c.policy_id,
        day: c.day,
        claim_insurer: c.insurer_id,
        bound_insurer: bound,
        detail: `policy ${c.policy_id} day ${c.day}: claim insurer ${c.insurer_id} not in panel`,
      });
    }
  }

  // Check 6
  for (const [subId, accDay] of subAcceptedDay) {
    if (accDay < maxDay && !subPolicy.has(subId)) {
      violations.push({
        check: "QuoteAcceptedWithoutPolicyBound",
        submission_id: subId,
        accepted_day: accDay,
        detail: `submission ${subId}: QuoteAccepted day ${accDay} has no PolicyBound`,
      });
    }
  }

  // Check 7
  for (const [subId, pid] of subPolicy) {
    const quoted = subLeaderQuoted.get(subId);
    const bound = policyLeader.get(pid);
    if (quoted !== undefined && bound !== undefined && quoted !== bound) {
      violations.push({
        check: "PolicyBoundInsurerMismatch",
        submission_id: subId,
        policy_id: pid,
        bound_insurer: bound,
        accepted_insurer: quoted,
        detail: `submission ${subId}: bound leader ${bound} ≠ accepted leader ${quoted}`,
      });
    }
  }

  // Check 8
  for (const [pid, count] of policyBindCount) {
    if (count > 1) {
      violations.push({
        check: "DuplicatePolicyBound",
        policy_id: pid,
        count,
        detail: `policy ${pid}: bound ${count} times`,
      });
    }
  }

  // Check 9
  for (const e of events) {
    if (e.type === "PolicyExpired" && !boundPolicies.has(e.data.policy_id)) {
      violations.push({
        check: "PolicyExpiredWithoutBound",
        policy_id: e.data.policy_id,
        detail: `policy ${e.data.policy_id}: PolicyExpired without prior PolicyBound`,
      });
    }
  }

  // Check 10
  for (const [key, reqDay] of leadRequested) {
    if (!leadResponses.has(key)) {
      const [subStr, insStr] = key.split("|");
      violations.push({
        check: "LeadQuoteOrphanRequest",
        submission_id: Number(subStr),
        insurer_id: Number(insStr),
        day: reqDay,
        detail: `sub ${subStr} insurer ${insStr} day ${reqDay}: request without response`,
      });
    }
  }

  // Check 11
  for (const [key, count] of leadResponses) {
    if (count > 1) {
      const [subStr, insStr] = key.split("|");
      violations.push({
        check: "LeadQuoteDuplicateResponse",
        submission_id: Number(subStr),
        insurer_id: Number(insStr),
        count,
        detail: `sub ${subStr} insurer ${insStr}: ${count} lead responses`,
      });
    }
  }

  // Check 12
  for (const o of orphanResponses) {
    violations.push({
      check: "LeadQuoteOrphanResponse",
      submission_id: o.submission_id,
      insurer_id: o.insurer_id,
      day: o.day,
      kind: o.kind,
      detail: `sub ${o.submission_id} insurer ${o.insurer_id} day ${o.day}: ${o.kind} without prior request`,
    });
  }

  return violations;
}

// ── Data prep ─────────────────────────────────────────────────────────────

export function prepPanel8Data(db, _opts = {}) {
  const events = db.events;
  const mechViolations = verifyMechanics(events);
  const intgViolations = verifyIntegrity(events);
  return {
    mechanics: buildSection(MECHANICS_CHECKS, mechViolations),
    integrity: buildSection(INTEGRITY_CHECKS, intgViolations),
  };
}

function buildSection(checks, violations) {
  const byCheck = new Map();
  for (const v of violations) {
    let bucket = byCheck.get(v.check);
    if (!bucket) { bucket = []; byCheck.set(v.check, bucket); }
    bucket.push(v);
  }
  return checks.map(([id, label]) => {
    const vs = byCheck.get(id) ?? [];
    return { id, label, status: vs.length === 0 ? "pass" : "fail", violations: vs };
  });
}

// ── Rendering ─────────────────────────────────────────────────────────────

const STYLE = `
.p8-wrap { font-variant-numeric: tabular-nums; }
.p8-section { margin-bottom: .75rem; }
.p8-section h3 { font-size: .8em; font-weight: 600; color: var(--fg-dim); margin: 0 0 .35rem 0; text-transform: uppercase; letter-spacing: .04em; }
.p8-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: .35rem; }
.p8-badge { border: 1px solid var(--panel-border); border-radius: 3px; padding: .35rem .5rem; font-size: .8em; cursor: default; display: flex; align-items: center; gap: .4rem; }
.p8-badge.p8-pass { color: var(--accent); border-color: var(--accent-dim); }
.p8-badge.p8-fail { color: var(--warn); border-color: var(--warn); cursor: pointer; }
.p8-badge .p8-dot { width: .55em; height: .55em; border-radius: 50%; background: currentColor; flex: none; }
.p8-badge .p8-label { flex: 1; }
.p8-badge .p8-count { color: var(--fg-dim); font-size: .85em; }
.p8-badge.p8-fail .p8-count { color: var(--warn); }
.p8-violations { margin: .25rem 0 .5rem 0; padding: .35rem .5rem; border: 1px solid var(--warn); border-radius: 3px; background: rgba(251,73,52,.06); font-size: .75em; max-height: 12em; overflow-y: auto; }
.p8-violations ul { margin: 0; padding-left: 1.2em; }
.p8-violations li { margin: .1em 0; color: var(--fg); }
.p8-summary { color: var(--fg-dim); font-size: .8em; margin-bottom: .5rem; }
.p8-summary .ok { color: var(--accent); }
.p8-summary .bad { color: var(--warn); }
`;

const MAX_VIOLATION_LINES = 25;

export function renderPanel8(data, opts = {}) {
  const asString = opts.asString === true;
  const html = buildHTML(data);
  if (asString) return html;
  const wrap = document.createElement("div");
  wrap.className = "p8-wrap";
  const styleEl = document.createElement("style");
  styleEl.textContent = STYLE;
  wrap.appendChild(styleEl);
  const host = document.createElement("div");
  host.innerHTML = html;
  while (host.firstChild) wrap.appendChild(host.firstChild);

  const expanded = new Set();
  wrap.querySelectorAll(".p8-badge.p8-fail").forEach((badge) => {
    badge.addEventListener("click", () => {
      const id = badge.getAttribute("data-id");
      const next = badge.nextElementSibling;
      if (next && next.classList.contains("p8-violations")) {
        next.remove();
        expanded.delete(id);
        return;
      }
      const section = [...data.mechanics, ...data.integrity].find((c) => c.id === id);
      if (!section) return;
      const list = document.createElement("div");
      list.className = "p8-violations";
      list.innerHTML = renderViolationList(section.violations);
      badge.after(list);
      expanded.add(id);
    });
  });
  return wrap;
}

function buildHTML(data) {
  const totalChecks = data.mechanics.length + data.integrity.length;
  const failedChecks = [...data.mechanics, ...data.integrity].filter((c) => c.status === "fail").length;
  const summary = failedChecks === 0
    ? `<div class="p8-summary"><span class="ok">all ${totalChecks} checks pass</span></div>`
    : `<div class="p8-summary"><span class="bad">${failedChecks} / ${totalChecks} checks failed</span> · click a red badge to inspect</div>`;
  return summary + sectionHTML("Mechanics", data.mechanics) + sectionHTML("Integrity", data.integrity);
}

function sectionHTML(title, checks) {
  const badges = checks.map(badgeHTML).join("");
  return `<div class="p8-section"><h3>${escapeHtml(title)}</h3><div class="p8-grid">${badges}</div></div>`;
}

function badgeHTML(c) {
  const cls = c.status === "pass" ? "p8-badge p8-pass" : "p8-badge p8-fail";
  const count = c.status === "pass" ? "PASS" : `FAIL · ${c.violations.length}`;
  return `<div class="${cls}" data-id="${escapeAttr(c.id)}"><span class="p8-dot"></span><span class="p8-label">${escapeHtml(c.label)}</span><span class="p8-count">${escapeHtml(count)}</span></div>`;
}

function renderViolationList(violations) {
  const shown = violations.slice(0, MAX_VIOLATION_LINES);
  const items = shown.map((v) => `<li>${escapeHtml(v.detail ?? JSON.stringify(v))}</li>`).join("");
  const more = violations.length > shown.length
    ? `<li>… ${violations.length - shown.length} more</li>`
    : "";
  return `<ul>${items}${more}</ul>`;
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]
  ));
}

function escapeAttr(s) {
  return escapeHtml(s);
}
