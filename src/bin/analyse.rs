//! Typed event-stream analyser for rins simulation output.
//!
//! Reads `events.ndjson` from the current directory, deserializes it using the
//! same `SimEvent` type the simulation writes, then prints:
//!   Tier 1  — 18 invariant status (PASS/FAIL per invariant: 6 mechanics, 12 integrity)
//!   Tier 2  — year-over-year character table (all columns guaranteed non-empty)

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
};

use rins::{
    analysis::{analyse, verify_integrity, verify_mechanics, IntegrityViolation, MechanicsViolation},
    config::SimulationConfig,
    events::SimEvent,
    types::InsurerId,
};

fn main() {
    // ── Resolve events file path: first positional arg, else default ──────────
    let events_path = std::env::args().nth(1).unwrap_or_else(|| "events.ndjson".to_string());

    // ── Load events ──────────────────────────────────────────────────────────
    let file = File::open(&events_path).unwrap_or_else(|e| {
        eprintln!("error: cannot open {events_path} — {e}");
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
    let invalid_df_fail =
        has(|v| matches!(v, MechanicsViolation::InvalidDamageFraction { .. }));

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
    println!(
        "  [{}] Inv 7 — LossEvent damage fraction in (0.0, 1.0]",
        status(invalid_df_fail)
    );

    if violations.is_empty() {
        println!("  All mechanics invariants PASS ({} events checked)", events.len());
    } else {
        println!("\n  {} violation(s) detected:", violations.len());
        for v in &violations {
            match v {
                MechanicsViolation::DayOffsetChain { submission_id, detail } => {
                    println!("    DayOffsetChain  sub={submission_id}  {detail}");
                }
                MechanicsViolation::LossBeforeBound { insured_id, loss_day, bound_day } => {
                    println!("    LossBeforeBound  insured={insured_id}  loss_day={loss_day}  bound_day={bound_day}");
                }
                MechanicsViolation::AttrNotStrictlyPostBound {
                    insured_id,
                    loss_day,
                    bound_day,
                } => {
                    println!("    AttrNotStrictlyPostBound  insured={insured_id}  loss_day={loss_day}  bound_day={bound_day}");
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
                MechanicsViolation::InvalidDamageFraction { event_id, damage_fraction } => {
                    println!("    InvalidDamageFraction  event={event_id}  df={damage_fraction}");
                }
            }
        }
    }

    println!();

    // ── Tier 1 continued: integrity invariants ────────────────────────────────
    let int_violations = verify_integrity(&events);

    println!("=== Tier 1 — Integrity Invariants ===");

    let ihas = |f: fn(&IntegrityViolation) -> bool| int_violations.iter().any(f);

    println!(
        "  [{}] Inv 8  — GUL ≤ sum insured (all perils)",
        status(ihas(|v| matches!(v, IntegrityViolation::GulExceedsSumInsured { .. })))
    );
    println!(
        "  [{}] Inv 9  — Aggregate claim ≤ sum insured per (policy, year)",
        status(ihas(|v| matches!(v, IntegrityViolation::AggregateClaimExceedsSumInsured { .. })))
    );
    println!(
        "  [{}] Inv 10 — Every ClaimSettled has a matching AssetDamage",
        status(ihas(|v| matches!(v, IntegrityViolation::ClaimWithoutMatchingLoss { .. })))
    );
    println!(
        "  [{}] Inv 11 — Claim amount > 0",
        status(ihas(|v| matches!(v, IntegrityViolation::ClaimAmountZero { .. })))
    );
    println!(
        "  [{}] Inv 12 — ClaimSettled insurer matches PolicyBound insurer",
        status(ihas(|v| matches!(v, IntegrityViolation::ClaimInsurerMismatch { .. })))
    );
    println!(
        "  [{}] Inv 13 — Every QuoteAccepted (non-final-day) has a PolicyBound",
        status(ihas(|v| matches!(v, IntegrityViolation::QuoteAcceptedWithoutPolicyBound { .. })))
    );
    println!(
        "  [{}] Inv 14 — PolicyBound insurer matches LeadQuoteIssued insurer",
        status(ihas(|v| matches!(v, IntegrityViolation::PolicyBoundInsurerMismatch { .. })))
    );
    println!(
        "  [{}] Inv 15 — No duplicate PolicyBound for same policy_id",
        status(ihas(|v| matches!(v, IntegrityViolation::DuplicatePolicyBound { .. })))
    );
    println!(
        "  [{}] Inv 16 — Every PolicyExpired references a bound policy",
        status(ihas(|v| matches!(v, IntegrityViolation::PolicyExpiredWithoutBound { .. })))
    );
    println!(
        "  [{}] Inv 17 — Every LeadQuoteRequested has exactly one insurer response",
        status(ihas(|v| matches!(v, IntegrityViolation::LeadQuoteOrphanRequest { .. })))
    );
    println!(
        "  [{}] Inv 18 — No duplicate insurer responses for same (submission, insurer)",
        status(ihas(|v| matches!(v, IntegrityViolation::LeadQuoteDuplicateResponse { .. })))
    );
    println!(
        "  [{}] Inv 19 — Every insurer response has a prior LeadQuoteRequested",
        status(ihas(|v| matches!(v, IntegrityViolation::LeadQuoteOrphanResponse { .. })))
    );

    if int_violations.is_empty() {
        println!("  All integrity invariants PASS");
    } else {
        println!("\n  {} integrity violation(s) detected:", int_violations.len());
        for v in &int_violations {
            println!("    {v}");
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

    const CENTS_PER_BUSD: f64 = 100_000_000_000.0; // cents per billion USD

    println!("=== Tier 2 — Year Character Table ===");
    println!(
        "{:>4} | {:>9} | {:>8} | {:>8} | {:>9} | {:>8} | {:>8} | {:>8} | {:>7} | {:>5} | {:>11} | {:>8} | {:>6} | {:>10} | {:>6}",
        "Year", "Assets(B)", "GUL(B)", "Cov(B)", "Claims(B)", "LossR%", "CombR%", "AvgCR3%", "Rate%", "Cats#", "TotalCap(B)", "Dropped#", "ApTp", "Insurers", "Gini"
    );
    println!("{}", "-".repeat(4 + 3 + 11 + 3 + 10 + 3 + 10 + 3 + 11 + 3 + 10 + 3 + 10 + 3 + 10 + 3 + 9 + 3 + 7 + 3 + 13 + 3 + 10 + 3 + 8 + 3 + 10 + 3 + 6));

    let mut recent_lrs: std::collections::VecDeque<f64> = std::collections::VecDeque::new();

    // ── Tier 3: premium dispersion ────────────────────────────────────────────
    // Group LeadQuoteIssued.premium by year, compute mean and population CV.
    // CV > 0.05 in a year confirms that insurers are pricing differently from
    // one another (capital depletion / own CR history active); CV ≈ 0 means
    // all insurers are quoting identically (new, no experience).
    {
        let mut by_year: std::collections::BTreeMap<u32, Vec<u64>> =
            std::collections::BTreeMap::new();
        for ev in &events {
            if let rins::events::Event::LeadQuoteIssued { premium, .. } = &ev.event {
                let year = (ev.day.0 / 360 + 1) as u32;
                by_year.entry(year).or_default().push(*premium);
            }
        }

        println!("=== Tier 3 — Premium Dispersion (CV of LeadQuoteIssued.premium per year) ===");
        println!(
            "{:>4} | {:>6} | {:>14} | {:>8}",
            "Year", "n", "AvgPrem(USD)", "CV"
        );
        println!("{}", "-".repeat(4 + 3 + 6 + 3 + 14 + 3 + 8));
        for (year, premiums) in &by_year {
            if premiums.len() < 2 {
                continue;
            }
            let n = premiums.len() as f64;
            let mean = premiums.iter().sum::<u64>() as f64 / n;
            let var = premiums.iter().map(|&p| (p as f64 - mean).powi(2)).sum::<f64>() / n;
            let cv = var.sqrt() / mean;
            println!(
                "{:>4} | {:>6} | {:>14.0} | {:>8.4}",
                year,
                premiums.len(),
                mean / 100.0, // cents → USD
                cv,
            );
        }
        println!();
    }

    for s in &stats {
        let lr_pct = s.loss_ratio() * 100.0;
        let cr_pct = s.combined_ratio(expense_ratio) * 100.0;
        let rol_pct = s.rate_on_line() * 100.0;
        let cap_b = s.total_capital as f64 / CENTS_PER_BUSD;
        let assets_b = s.total_assets as f64 / CENTS_PER_BUSD;
        let gul_b = (s.attr_gul + s.cat_gul) as f64 / CENTS_PER_BUSD;
        let cov_b = s.sum_insured as f64 / CENTS_PER_BUSD;
        let claims_b = s.claims as f64 / CENTS_PER_BUSD;
        let lr = if s.bound_premium > 0 { s.claims as f64 / s.bound_premium as f64 } else { 0.0 };
        recent_lrs.push_back(lr);
        if recent_lrs.len() > 3 { recent_lrs.pop_front(); }
        let n = recent_lrs.len();
        let avg_cr_pct: Option<f64> = if n >= 2 {
            let avg_lr = recent_lrs.iter().sum::<f64>() / n as f64;
            Some((avg_lr + expense_ratio) * 100.0)
        } else {
            None
        };
        let avg_cr_str = match avg_cr_pct {
            Some(v) => format!("{:>7.1}%", v),
            None => "   n/a  ".to_string(),
        };
        let ap_tp_str = if n >= 2 {
            let avg_lr = recent_lrs.iter().sum::<f64>() / n as f64;
            let avg_cr = avg_lr + expense_ratio;
            let cr_signal = (avg_cr - 1.0_f64).clamp(-0.50, 0.80);
            let capacity_uplift = if s.dropped_count > 10 { 0.05 } else { 0.0 };
            let factor = 1.0 + cr_signal + capacity_uplift;
            format!("{:>6.2}", factor)
        } else {
            "  n/a ".to_string()
        };
        let insurer_str = {
            let base = format!("{}", s.insurer_count);
            let plus = s.entrant_count;
            let minus = s.insolvent_count;
            match (plus > 0, minus > 0) {
                (true, true)  => format!("{base} +{plus}-{minus}"),
                (true, false) => format!("{base} +{plus}"),
                (false, true) => format!("{base} -{minus}"),
                (false, false) => base,
            }
        };
        println!(
            "{:>4} | {:>9.2} | {:>8.2} | {:>8.2} | {:>9.2} | {:>7.1}% | {:>7.1}% | {} | {:>6.2}% | {:>5} | {:>11.2} | {:>8} | {} | {} | {:>6.3}",
            s.year,
            assets_b,
            gul_b,
            cov_b,
            claims_b,
            lr_pct,
            cr_pct,
            avg_cr_str,
            rol_pct,
            s.cat_event_count,
            cap_b,
            s.dropped_count,
            ap_tp_str,
            insurer_str,
            s.gini_market_share,
        );
    }
    println!();
}
