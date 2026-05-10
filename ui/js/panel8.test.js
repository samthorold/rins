import { test } from "node:test";
import assert from "node:assert/strict";

import { parseNDJSONText } from "./data.js";
import {
  verifyMechanics,
  verifyIntegrity,
  prepPanel8Data,
  renderPanel8,
  MECHANICS_CHECKS,
  INTEGRITY_CHECKS,
} from "./panel8.js";

// Build a minimal valid event stream: one insured, one submission, full chain.
// Day 0: SimulationStart, InsurerEntered (1, 2), CoverageRequested
// Day 1: LeadQuoteRequested (sub=0, ins=1) → LeadQuoteIssued
//        FollowerQuoteRequested (sub=0, ins=2) → FollowerQuoteIssued
// Day 2: QuoteAccepted (leader=1)
// Day 3: PolicyBound (policy=0)
// Day 363: PolicyExpired (= QA day + 361)
function validStream(extra = []) {
  const ev = [
    [0, { SimulationStart: { year_start: 1, warmup_years: 0, analysis_years: 2 } }],
    [0, { YearStart: { year: 1 } }],
    [0, { InsurerEntered: { insurer_id: 1, initial_capital: 1000 } }],
    [0, { InsurerEntered: { insurer_id: 2, initial_capital: 1000 } }],
    [0, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: ["WindstormAtlantic", "Attritional"] } } }],
    [1, { LeadQuoteRequested: { submission_id: 0, insured_id: 1, insurer_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: ["WindstormAtlantic", "Attritional"] } } }],
    [1, { LeadQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } }],
    [1, { FollowerQuoteRequested: { submission_id: 0, insured_id: 1, insurer_id: 2, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: ["WindstormAtlantic", "Attritional"] }, lead_premium: 100, lead_atp: 100 } }],
    [1, { FollowerQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 2, line_size: 0.75 } }],
    [2, { QuoteAccepted: { submission_id: 0, insured_id: 1, leader_id: 1, panel: [[1, 0.25], [2, 0.75]], premium: 100 } }],
    [3, { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[1, 0.25], [2, 0.75]], premium: 100, sum_insured: 1000 } }],
    [363, { PolicyExpired: { policy_id: 0 } }],
    ...extra,
  ];
  return ev.map(([day, event]) => JSON.stringify({ day, event })).join("\n");
}

function parse(stream) {
  return parseNDJSONText(stream).events;
}

// ── Mechanics ──────────────────────────────────────────────────────────────

test("MECHANICS_CHECKS exposes 7 checks", () => {
  assert.equal(MECHANICS_CHECKS.length, 7);
});

test("verifyMechanics — valid stream has zero violations", () => {
  const v = verifyMechanics(parse(validStream()));
  assert.deepEqual(v, []);
});

test("verifyMechanics — DayOffsetChain flags PolicyBound at wrong offset", () => {
  // Replace day-3 PolicyBound with day-5 PolicyBound and shift expiry to keep PolicyExpired timing valid.
  const ev = [
    [0, { SimulationStart: { year_start: 1, warmup_years: 0, analysis_years: 2 } }],
    [0, { YearStart: { year: 1 } }],
    [0, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: ["WindstormAtlantic"] } } }],
    [1, { LeadQuoteRequested: { submission_id: 0, insured_id: 1, insurer_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
    [1, { LeadQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 1 } }],
    [2, { QuoteAccepted: { submission_id: 0, insured_id: 1, leader_id: 1, panel: [[1, 1]], premium: 100 } }],
    [5, { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[1, 1]], premium: 100, sum_insured: 1000 } }],
  ].map(([d, e]) => JSON.stringify({ day: d, event: e })).join("\n");
  const v = verifyMechanics(parse(ev));
  assert.ok(v.some((x) => x.check === "DayOffsetChain"), "should flag DayOffsetChain");
});

test("verifyMechanics — LossBeforeBound flags AssetDamage before first CR", () => {
  const stream = validStream([
    [0, { CoverageRequested: { insured_id: 9, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
  ]);
  // Insert an early AssetDamage by hand for an *earlier* insured first-CR day:
  // Use insured 1 (first CR at day 0), AssetDamage at day -? — can't be negative.
  // Instead use a fresh insured whose first CR is day 100 but with damage at day 50.
  const ev = stream + "\n" + JSON.stringify({ day: 50, event: { AssetDamage: { insured_id: 99, peril: "WindstormAtlantic", ground_up_loss: 10 } } })
    + "\n" + JSON.stringify({ day: 100, event: { CoverageRequested: { insured_id: 99, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } } });
  const v = verifyMechanics(parse(ev));
  assert.ok(v.some((x) => x.check === "LossBeforeBound"));
});

test("verifyMechanics — AttrNotStrictlyPostBound flags Attritional on bound day", () => {
  const stream = validStream([
    [0, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 5 } }],
  ]);
  const v = verifyMechanics(parse(stream));
  assert.ok(v.some((x) => x.check === "AttrNotStrictlyPostBound"));
});

test("verifyMechanics — PolicyExpiredTiming flags wrong expiry day", () => {
  const ev = [
    [0, { SimulationStart: { year_start: 1, warmup_years: 0, analysis_years: 2 } }],
    [0, { YearStart: { year: 1 } }],
    [0, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
    [1, { LeadQuoteRequested: { submission_id: 0, insured_id: 1, insurer_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
    [1, { LeadQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 1 } }],
    [2, { QuoteAccepted: { submission_id: 0, insured_id: 1, leader_id: 1, panel: [[1, 1]], premium: 100 } }],
    [3, { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[1, 1]], premium: 100, sum_insured: 1000 } }],
    [400, { PolicyExpired: { policy_id: 0 } }],
  ].map(([d, e]) => JSON.stringify({ day: d, event: e })).join("\n");
  const v = verifyMechanics(parse(ev));
  assert.ok(v.some((x) => x.check === "PolicyExpiredTiming"));
});

test("verifyMechanics — ClaimAfterExpiry flags claim after PolicyExpired", () => {
  const stream = validStream([
    [400, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 10, peril: "Attritional", remaining_capital: 100 } }],
  ]);
  const v = verifyMechanics(parse(stream));
  assert.ok(v.some((x) => x.check === "ClaimAfterExpiry"));
});

test("verifyMechanics — CatFractionInconsistent flags GUL > sum_insured for cat", () => {
  const stream = validStream([
    [50, { AssetDamage: { insured_id: 1, peril: "WindstormAtlantic", ground_up_loss: 9999 } }],
  ]);
  const v = verifyMechanics(parse(stream));
  assert.ok(v.some((x) => x.check === "CatFractionInconsistent"));
});

test("verifyMechanics — InvalidDamageFraction flags df out of (0, 1]", () => {
  const stream = validStream([
    [50, { LossEvent: { event_id: 0, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 1.5 } }],
    [60, { LossEvent: { event_id: 1, peril: "WindstormAtlantic", territory: "US-NE", damage_fraction: 0 } }],
  ]);
  const v = verifyMechanics(parse(stream));
  assert.equal(v.filter((x) => x.check === "InvalidDamageFraction").length, 2);
});

// ── Integrity ──────────────────────────────────────────────────────────────

test("INTEGRITY_CHECKS exposes 12 checks", () => {
  assert.equal(INTEGRITY_CHECKS.length, 12);
});

test("verifyIntegrity — valid stream has zero violations", () => {
  const v = verifyIntegrity(parse(validStream()));
  assert.deepEqual(v, []);
});

test("verifyIntegrity — GulExceedsSumInsured flags oversized gul", () => {
  const stream = validStream([
    [50, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 9999 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "GulExceedsSumInsured"));
});

test("verifyIntegrity — AggregateClaimExceedsSumInsured flags excessive claims", () => {
  const stream = validStream([
    [50, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 600 } }],
    [50, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 600, peril: "Attritional", remaining_capital: 100 } }],
    [60, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 600 } }],
    [60, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 600, peril: "Attritional", remaining_capital: 100 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "AggregateClaimExceedsSumInsured"));
});

test("verifyIntegrity — ClaimWithoutMatchingLoss flags orphan claim", () => {
  const stream = validStream([
    [50, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 10, peril: "Attritional", remaining_capital: 100 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "ClaimWithoutMatchingLoss"));
});

test("verifyIntegrity — ClaimAmountZero flags zero-amount claim", () => {
  const stream = validStream([
    [50, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 10 } }],
    [50, { ClaimSettled: { policy_id: 0, insurer_id: 1, amount: 0, peril: "Attritional", remaining_capital: 100 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "ClaimAmountZero"));
});

test("verifyIntegrity — ClaimInsurerMismatch flags claim from non-panel insurer", () => {
  const stream = validStream([
    [50, { AssetDamage: { insured_id: 1, peril: "Attritional", ground_up_loss: 10 } }],
    [50, { ClaimSettled: { policy_id: 0, insurer_id: 99, amount: 10, peril: "Attritional", remaining_capital: 100 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "ClaimInsurerMismatch"));
});

test("verifyIntegrity — QuoteAcceptedWithoutPolicyBound flags missing bind", () => {
  // QuoteAccepted not on max day, but no PolicyBound for it.
  const ev = [
    [0, { SimulationStart: { year_start: 1, warmup_years: 0, analysis_years: 2 } }],
    [0, { YearStart: { year: 1 } }],
    [0, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
    [1, { LeadQuoteRequested: { submission_id: 0, insured_id: 1, insurer_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
    [1, { LeadQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 1 } }],
    [2, { QuoteAccepted: { submission_id: 0, insured_id: 1, leader_id: 1, panel: [[1, 1]], premium: 100 } }],
    [500, { YearEnd: { year: 1 } }],  // pushes max_day past the QuoteAccepted day
  ].map(([d, e]) => JSON.stringify({ day: d, event: e })).join("\n");
  const v = verifyIntegrity(parse(ev));
  assert.ok(v.some((x) => x.check === "QuoteAcceptedWithoutPolicyBound"));
});

test("verifyIntegrity — PolicyBoundInsurerMismatch flags wrong leader", () => {
  const ev = [
    [0, { SimulationStart: { year_start: 1, warmup_years: 0, analysis_years: 2 } }],
    [0, { YearStart: { year: 1 } }],
    [0, { CoverageRequested: { insured_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
    [1, { LeadQuoteRequested: { submission_id: 0, insured_id: 1, insurer_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
    [1, { LeadQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 1 } }],
    [2, { QuoteAccepted: { submission_id: 0, insured_id: 1, leader_id: 1, panel: [[1, 1]], premium: 100 } }],
    [3, { PolicyBound: { policy_id: 0, submission_id: 0, insured_id: 1, panel: [[42, 1]], premium: 100, sum_insured: 1000 } }],
  ].map(([d, e]) => JSON.stringify({ day: d, event: e })).join("\n");
  const v = verifyIntegrity(parse(ev));
  assert.ok(v.some((x) => x.check === "PolicyBoundInsurerMismatch"));
});

test("verifyIntegrity — DuplicatePolicyBound flags repeat bind", () => {
  const stream = validStream([
    [3, { PolicyBound: { policy_id: 0, submission_id: 1, insured_id: 1, panel: [[1, 1]], premium: 100, sum_insured: 1000 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "DuplicatePolicyBound"));
});

test("verifyIntegrity — PolicyExpiredWithoutBound flags orphan expiry", () => {
  const stream = validStream([
    [400, { PolicyExpired: { policy_id: 999 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "PolicyExpiredWithoutBound"));
});

test("verifyIntegrity — LeadQuoteOrphanRequest flags request without response", () => {
  const stream = validStream([
    [50, { LeadQuoteRequested: { submission_id: 99, insured_id: 1, insurer_id: 1, risk: { sum_insured: 1000, territory: "US-NE", perils_covered: [] } } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "LeadQuoteOrphanRequest"));
});

test("verifyIntegrity — LeadQuoteDuplicateResponse flags two responses", () => {
  const stream = validStream([
    [1, { LeadQuoteIssued: { submission_id: 0, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "LeadQuoteDuplicateResponse"));
});

test("verifyIntegrity — LeadQuoteOrphanResponse flags response without request", () => {
  const stream = validStream([
    [50, { LeadQuoteIssued: { submission_id: 88, insured_id: 1, insurer_id: 1, atp: 100, premium: 100, cat_exposure_at_quote: 0, line_size: 0.25 } }],
  ]);
  const v = verifyIntegrity(parse(stream));
  assert.ok(v.some((x) => x.check === "LeadQuoteOrphanResponse"));
});

// ── prepPanel8Data + render ───────────────────────────────────────────────

test("prepPanel8Data — returns mechanics + integrity sections with status per check", () => {
  const db = parseNDJSONText(validStream());
  const data = prepPanel8Data(db);
  assert.equal(data.mechanics.length, 7);
  assert.equal(data.integrity.length, 12);
  for (const c of [...data.mechanics, ...data.integrity]) {
    assert.ok(typeof c.id === "string");
    assert.ok(typeof c.label === "string");
    assert.equal(c.status, "pass");
    assert.deepEqual(c.violations, []);
  }
});

test("prepPanel8Data — failing check carries violations", () => {
  const db = parseNDJSONText(validStream([
    [400, { PolicyExpired: { policy_id: 999 } }],
  ]));
  const data = prepPanel8Data(db);
  const orphan = data.integrity.find((c) => c.id === "PolicyExpiredWithoutBound");
  assert.equal(orphan.status, "fail");
  assert.ok(orphan.violations.length > 0);
});

test("renderPanel8 — emits 19 badges with pass/fail classes", () => {
  const db = parseNDJSONText(validStream([
    [400, { PolicyExpired: { policy_id: 999 } }],
  ]));
  const data = prepPanel8Data(db);
  const html = renderPanel8(data, { asString: true });
  assert.equal((html.match(/class="p8-badge/g) ?? []).length, 19);
  assert.ok(/p8-badge p8-fail[^"]*"[^>]*data-id="PolicyExpiredWithoutBound"/.test(html));
});
