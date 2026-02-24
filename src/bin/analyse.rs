//! Typed event-stream analyser for rins simulation output.
//!
//! Reads `events.ndjson` from the current directory, deserializes it using the
//! same `SimEvent` type the simulation writes, then prints:
//!   Tier 1  — mechanics invariant status (PASS/FAIL per invariant)
//!   Tier 2  — year-over-year character table (all columns guaranteed non-empty)

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
};

use rins::{
    analysis::{analyse, verify_mechanics, MechanicsViolation},
    config::SimulationConfig,
    events::SimEvent,
    types::InsurerId,
};

fn main() {
    // ── Load events ──────────────────────────────────────────────────────────
    let file = File::open("events.ndjson").unwrap_or_else(|e| {
        eprintln!("error: cannot open events.ndjson — {e}");
        eprintln!("Run `cargo run --release` first to generate the event stream.");
        std::process::exit(1);
    });

    let mut events: Vec<SimEvent> = Vec::new();
    for (line_no, line) in BufReader::new(file).lines().enumerate() {
        let line = line.unwrap_or_else(|e| {
            eprintln!("error reading line {}: {}", line_no + 1, e);
            std::process::exit(1);
        });
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<SimEvent>(&line) {
            Ok(ev) => events.push(ev),
            Err(e) => {
                eprintln!("error: failed to deserialize line {}: {}", line_no + 1, e);
                eprintln!("  line: {line}");
                std::process::exit(1);
            }
        }
    }

    // ── Build initial capitals from canonical config ──────────────────────────
    let config = SimulationConfig::canonical();
    let initial_capitals: HashMap<InsurerId, u64> = config
        .insurers
        .iter()
        .map(|ic| (ic.id, ic.initial_capital as u64))
        .collect();

    let expense_ratio = config
        .insurers
        .first()
        .map(|ic| ic.expense_ratio)
        .unwrap_or(0.344);

    // ── Tier 1: mechanics invariants ─────────────────────────────────────────
    let violations = verify_mechanics(&events);

    println!("=== Tier 1 — Mechanics Invariants ===");

    // Each invariant is a category; FAIL if any violation of that kind exists.
    let has = |f: fn(&MechanicsViolation) -> bool| violations.iter().any(f);

    let offset_fail = has(|v| matches!(v, MechanicsViolation::DayOffsetChain { .. }));
    let loss_before_fail = has(|v| matches!(v, MechanicsViolation::LossBeforeBound { .. }));
    let attr_strict_fail =
        has(|v| matches!(v, MechanicsViolation::AttrNotStrictlyPostBound { .. }));
    let expiry_timing_fail =
        has(|v| matches!(v, MechanicsViolation::PolicyExpiredTiming { .. }));
    let claim_after_fail = has(|v| matches!(v, MechanicsViolation::ClaimAfterExpiry { .. }));
    let cat_frac_fail =
        has(|v| matches!(v, MechanicsViolation::CatFractionInconsistent { .. }));

    fn status(fail: bool) -> &'static str {
        if fail { "FAIL" } else { "PASS" }
    }

    println!(
        "  [{}] Inv 1 — Day offset chain (PolicyBound = LeadQuoteRequested + 2)",
        status(offset_fail)
    );
    println!(
        "  [{}] Inv 2 — No loss before policy bound",
        status(loss_before_fail)
    );
    println!(
        "  [{}] Inv 3 — Attritional loss strictly after bound",
        status(attr_strict_fail)
    );
    println!(
        "  [{}] Inv 4 — PolicyExpired = QuoteAccepted + 361",
        status(expiry_timing_fail)
    );
    println!(
        "  [{}] Inv 5 — No claim after policy expiry",
        status(claim_after_fail)
    );
    println!(
        "  [{}] Inv 6 — Cat GUL ≤ sum insured (damage fraction ≤ 1.0)",
        status(cat_frac_fail)
    );

    if violations.is_empty() {
        println!("  All invariants PASS ({} events checked)", events.len());
    } else {
        println!("\n  {} violation(s) detected:", violations.len());
        for v in &violations {
            match v {
                MechanicsViolation::DayOffsetChain { submission_id, detail } => {
                    println!("    DayOffsetChain  sub={submission_id}  {detail}");
                }
                MechanicsViolation::LossBeforeBound { policy_id, loss_day, bound_day } => {
                    println!("    LossBeforeBound  policy={policy_id}  loss_day={loss_day}  bound_day={bound_day}");
                }
                MechanicsViolation::AttrNotStrictlyPostBound {
                    policy_id,
                    loss_day,
                    bound_day,
                } => {
                    println!("    AttrNotStrictlyPostBound  policy={policy_id}  loss_day={loss_day}  bound_day={bound_day}");
                }
                MechanicsViolation::PolicyExpiredTiming { policy_id, expected, actual } => {
                    println!("    PolicyExpiredTiming  policy={policy_id}  expected={expected}  actual={actual}");
                }
                MechanicsViolation::ClaimAfterExpiry { policy_id, claim_day, expiry_day } => {
                    println!("    ClaimAfterExpiry  policy={policy_id}  claim_day={claim_day}  expiry_day={expiry_day}");
                }
                MechanicsViolation::CatFractionInconsistent { peril, day, detail } => {
                    println!("    CatFractionInconsistent  peril={peril}  day={day}  {detail}");
                }
            }
        }
    }

    println!();

    // ── Tier 2: year character table ─────────────────────────────────────────
    let (_warmup, stats) = analyse(&events, &initial_capitals, expense_ratio);

    if stats.is_empty() {
        println!("=== Tier 2 — Year Character Table ===");
        println!("  (no analysis years in event stream)");
        return;
    }

    println!("=== Tier 2 — Year Character Table ===");
    println!(
        "{:>4} | {:>8} | {:>8} | {:>7} | {:<16} | {:>11} | {:>10}",
        "Year", "LossR%", "CombR%", "Rate%", "Dominant Peril", "TotalCap(B)", "Insolvent#"
    );
    println!("{}", "-".repeat(4 + 3 + 10 + 3 + 10 + 3 + 9 + 3 + 18 + 3 + 13 + 3 + 12));

    for s in &stats {
        let lr_pct = s.loss_ratio() * 100.0;
        let cr_pct = s.combined_ratio(expense_ratio) * 100.0;
        let rol_pct = s.rate_on_line() * 100.0;
        let cap_b = s.total_capital as f64 / 100.0 / 1e9; // cents → USD → billions
        println!(
            "{:>4} | {:>7.1}% | {:>7.1}% | {:>6.2}% | {:<16} | {:>11.2} | {:>10}",
            s.year,
            lr_pct,
            cr_pct,
            rol_pct,
            s.dominant_peril(),
            cap_b,
            s.insolvent_count,
        );
    }
    println!();
}
