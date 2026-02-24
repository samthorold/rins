use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};

use rins::analysis::{self, IntegrityViolation, MechanicsViolation};
use rins::config::SimulationConfig;
use rins::simulation::Simulation;
use rins::types::InsurerId;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut seed_override: Option<u64> = None;
    let mut output_path = "events.ndjson".to_string();
    let mut quiet = false;
    let mut runs: Option<u64> = None;
    let mut output_dir = ".".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => {
                i += 1;
                seed_override = Some(args[i].parse().expect("--seed requires a u64"));
            }
            "--output" => {
                i += 1;
                output_path = args[i].clone();
            }
            "--quiet" => quiet = true,
            "--runs" => {
                i += 1;
                runs = Some(args[i].parse().expect("--runs requires a positive integer"));
            }
            "--output-dir" => {
                i += 1;
                output_dir = args[i].clone();
            }
            _ => {}
        }
        i += 1;
    }

    let base_config = SimulationConfig::canonical();
    let start_seed = seed_override.unwrap_or(base_config.seed);

    if let Some(n) = runs {
        use rayon::prelude::*;

        std::fs::create_dir_all(&output_dir).expect("failed to create output directory");
        (0u64..n).into_par_iter().for_each(|i| {
            let seed = start_seed + i;
            let mut config = base_config.clone();
            config.seed = seed;
            let mut sim = Simulation::from_config(config);
            sim.start();
            sim.run();
            let path = format!("{}/events_seed_{}.ndjson", output_dir, seed);
            let file = File::create(&path)
                .unwrap_or_else(|e| panic!("failed to create {path}: {e}"));
            let mut writer = BufWriter::new(file);
            for ev in &sim.log {
                serde_json::to_writer(&mut writer, ev).expect("serialize");
                writeln!(writer).expect("newline");
            }
            if !quiet {
                println!("Seed {seed}: {} events → {path}", sim.log.len());
            }
        });
    } else {
        // Extract analysis inputs before base_config is moved.
        let initial_capitals: HashMap<InsurerId, u64> = base_config
            .insurers
            .iter()
            .map(|ic| (ic.id, ic.initial_capital.max(0) as u64))
            .collect();
        let expense_ratio =
            base_config.insurers.first().map(|ic| ic.expense_ratio).unwrap_or(0.344);

        let mut config = base_config;
        config.seed = start_seed;

        let mut sim = Simulation::from_config(config);

        sim.start();
        sim.run();

        let file = File::create(&output_path).expect("failed to create output file");
        let mut writer = BufWriter::new(file);
        for e in &sim.log {
            serde_json::to_writer(&mut writer, e).expect("failed to serialize event");
            writeln!(writer).expect("failed to write newline");
        }

        if !quiet {
            println!("Events fired: {}", sim.log.len());
            print_analysis(&sim.log, &initial_capitals, expense_ratio);
        }
    }
}

fn print_analysis(
    log: &[rins::events::SimEvent],
    initial_capitals: &HashMap<InsurerId, u64>,
    expense_ratio: f64,
) {
    // ── Mechanics invariants ──────────────────────────────────────────────────
    let violations = analysis::verify_mechanics(log);

    let inv = |variant: fn(&MechanicsViolation) -> bool| {
        if violations.iter().any(variant) { "FAIL" } else { "PASS" }
    };

    println!("\n=== Mechanics invariants ===");
    println!("  [1] Day-offset chain:               {}", inv(|v| matches!(v, MechanicsViolation::DayOffsetChain { .. })));
    println!("  [2] Loss before bound:               {}", inv(|v| matches!(v, MechanicsViolation::LossBeforeBound { .. })));
    println!("  [3] Attritional strictly post-bound: {}", inv(|v| matches!(v, MechanicsViolation::AttrNotStrictlyPostBound { .. })));
    println!("  [4] PolicyExpired timing:            {}", inv(|v| matches!(v, MechanicsViolation::PolicyExpiredTiming { .. })));
    println!("  [5] Claim after expiry:              {}", inv(|v| matches!(v, MechanicsViolation::ClaimAfterExpiry { .. })));
    println!("  [6] Cat fraction consistency:        {}", inv(|v| matches!(v, MechanicsViolation::CatFractionInconsistent { .. })));

    if violations.is_empty() {
        println!("  All mechanics invariants: PASS");
    } else {
        println!("\n  {} violation(s):", violations.len());
        for v in &violations {
            println!("    {v}");
        }
    }

    let int_violations = analysis::verify_integrity(log);
    let iinv = |variant: fn(&IntegrityViolation) -> bool| {
        if int_violations.iter().any(variant) { "FAIL" } else { "PASS" }
    };
    println!("\n=== Integrity invariants ===");
    println!("  [7]  GUL ≤ sum insured (all perils):                          {}", iinv(|v| matches!(v, IntegrityViolation::GulExceedsSumInsured { .. })));
    println!("  [8]  Aggregate claim ≤ sum insured per (policy, year):        {}", iinv(|v| matches!(v, IntegrityViolation::AggregateClaimExceedsSumInsured { .. })));
    println!("  [9]  Every ClaimSettled has matching AssetDamage:              {}", iinv(|v| matches!(v, IntegrityViolation::ClaimWithoutMatchingLoss { .. })));
    println!("  [10] Claim amount > 0:                                         {}", iinv(|v| matches!(v, IntegrityViolation::ClaimAmountZero { .. })));
    println!("  [11] ClaimSettled insurer matches PolicyBound insurer:         {}", iinv(|v| matches!(v, IntegrityViolation::ClaimInsurerMismatch { .. })));
    println!("  [12] Every QuoteAccepted (non-final-day) has PolicyBound:      {}", iinv(|v| matches!(v, IntegrityViolation::QuoteAcceptedWithoutPolicyBound { .. })));
    println!("  [13] PolicyBound insurer matches LeadQuoteIssued insurer:      {}", iinv(|v| matches!(v, IntegrityViolation::PolicyBoundInsurerMismatch { .. })));
    println!("  [14] No duplicate PolicyBound for same policy_id:              {}", iinv(|v| matches!(v, IntegrityViolation::DuplicatePolicyBound { .. })));
    println!("  [15] Every PolicyExpired references a bound policy:            {}", iinv(|v| matches!(v, IntegrityViolation::PolicyExpiredWithoutBound { .. })));
    if int_violations.is_empty() {
        println!("  All integrity invariants: PASS");
    } else {
        println!("\n  {} violation(s):", int_violations.len());
        for v in &int_violations {
            println!("    {v}");
        }
    }

    // ── Year character table ──────────────────────────────────────────────────
    let (warmup, stats) = analysis::analyse(log, initial_capitals, expense_ratio);

    if stats.is_empty() {
        return;
    }

    let last_year = stats.last().map(|s| s.year).unwrap_or(0);
    println!(
        "\n=== Year character table (warmup: {warmup}, analysis: years {}–{last_year}) ===",
        warmup + 1
    );
    println!(
        "{:>4} | {:>9} | {:>8} | {:>8} | {:>9} | {:>8} | {:>8} | {:>7} | {:<16} | {:>11} | {:>10} | {:>8}",
        "Year", "Assets(B)", "GUL(B)", "Cov(B)", "Claims(B)", "LossR%", "CombR%", "Rate%", "Dominant Peril", "TotalCap(B)", "Insolvent#", "Dropped#"
    );
    println!("{}", "-".repeat(4 + 3 + 11 + 3 + 10 + 3 + 10 + 3 + 11 + 3 + 10 + 3 + 10 + 3 + 9 + 3 + 18 + 3 + 13 + 3 + 12 + 3 + 10));

    const CENTS_PER_BUSD: f64 = 100_000_000_000.0; // cents per billion USD

    for s in &stats {
        let assets_b = s.total_assets as f64 / CENTS_PER_BUSD;
        let gul_b = (s.attr_gul + s.cat_gul) as f64 / CENTS_PER_BUSD;
        let cov_b = s.sum_insured as f64 / CENTS_PER_BUSD;
        let claims_b = s.claims as f64 / CENTS_PER_BUSD;
        println!(
            "{:>4} | {:>9.2} | {:>8.2} | {:>8.2} | {:>9.2} | {:>7.1}% | {:>7.1}% | {:>6.2}% | {:<16} | {:>11.2} | {:>10} | {:>8}",
            s.year,
            assets_b,
            gul_b,
            cov_b,
            claims_b,
            s.loss_ratio() * 100.0,
            s.combined_ratio(expense_ratio) * 100.0,
            s.rate_on_line() * 100.0,
            s.dominant_peril(),
            s.total_capital as f64 / CENTS_PER_BUSD,
            s.insolvent_count,
            s.dropped_count,
        );
    }
}
