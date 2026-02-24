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
    let mut years_override: Option<u32> = None;
    let mut output_path = "events.ndjson".to_string();
    let mut quiet = false;
    let mut runs: Option<u64> = None;
    let mut output_dir_opt: Option<String> = None;
    let mut csv_path_opt: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => {
                i += 1;
                seed_override = Some(args[i].parse().expect("--seed requires a u64"));
            }
            "--years" => {
                i += 1;
                years_override = Some(args[i].parse().expect("--years requires a u32"));
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
                output_dir_opt = Some(args[i].clone());
            }
            "--csv" => {
                i += 1;
                csv_path_opt = Some(args[i].clone());
            }
            _ => {}
        }
        i += 1;
    }

    let mut base_config = SimulationConfig::canonical();
    let start_seed = seed_override.unwrap_or(base_config.seed);
    if let Some(y) = years_override {
        base_config.years = y;
    }

    // Extract analysis inputs before base_config is (potentially) moved.
    let initial_capitals: HashMap<InsurerId, u64> = base_config
        .insurers
        .iter()
        .map(|ic| (ic.id, ic.initial_capital.max(0) as u64))
        .collect();
    let expense_ratio =
        base_config.insurers.first().map(|ic| ic.expense_ratio).unwrap_or(0.344);

    if let Some(n) = runs {
        use rayon::prelude::*;

        if let Some(ref dir) = output_dir_opt {
            std::fs::create_dir_all(dir).expect("failed to create output directory");
        }

        let all_stats: Vec<Vec<rins::analysis::YearStats>> = (0u64..n)
            .into_par_iter()
            .map(|i| {
                let seed = start_seed + i;
                let mut config = base_config.clone();
                config.seed = seed;
                let mut sim = Simulation::from_config(config);
                sim.start();
                sim.run();

                if let Some(ref dir) = output_dir_opt {
                    let path = format!("{dir}/events_seed_{seed}.ndjson");
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
                }

                analysis::analyse(&sim.log, &initial_capitals, expense_ratio).1
            })
            .collect();

        if let Some(ref csv_path) = csv_path_opt {
            write_runs_csv(&all_stats, start_seed, expense_ratio, csv_path);
        }

        if !quiet {
            print_all_run_years(&all_stats, start_seed, expense_ratio);
            if n < 2 {
                eprintln!("Warning: Distribution requires >= 2 runs");
            } else {
                let dists = analysis::analyse_distributions(&all_stats, expense_ratio);
                print_distributions(&dists, n);
            }
        }
    } else {
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
        "{:>4} | {:>9} | {:>8} | {:>8} | {:>9} | {:>8} | {:>8} | {:>7} | {:>5} | {:>11} | {:>10} | {:>8} | {:>9}",
        "Year", "Assets(B)", "GUL(B)", "Cov(B)", "Claims(B)", "LossR%", "CombR%", "Rate%", "Cats#", "TotalCap(B)", "Insolvent#", "Dropped#", "Entrants#"
    );
    println!("{}", "-".repeat(4 + 3 + 11 + 3 + 10 + 3 + 10 + 3 + 11 + 3 + 10 + 3 + 10 + 3 + 9 + 3 + 7 + 3 + 13 + 3 + 12 + 3 + 10 + 3 + 11));

    const CENTS_PER_BUSD: f64 = 100_000_000_000.0; // cents per billion USD

    for s in &stats {
        let assets_b = s.total_assets as f64 / CENTS_PER_BUSD;
        let gul_b = (s.attr_gul + s.cat_gul) as f64 / CENTS_PER_BUSD;
        let cov_b = s.sum_insured as f64 / CENTS_PER_BUSD;
        let claims_b = s.claims as f64 / CENTS_PER_BUSD;
        println!(
            "{:>4} | {:>9.2} | {:>8.2} | {:>8.2} | {:>9.2} | {:>7.1}% | {:>7.1}% | {:>6.2}% | {:>5} | {:>11.2} | {:>10} | {:>8} | {:>9}",
            s.year,
            assets_b,
            gul_b,
            cov_b,
            claims_b,
            s.loss_ratio() * 100.0,
            s.combined_ratio(expense_ratio) * 100.0,
            s.rate_on_line() * 100.0,
            s.cat_event_count,
            s.total_capital as f64 / CENTS_PER_BUSD,
            s.insolvent_count,
            s.dropped_count,
            s.entrant_count,
        );
    }
}

fn write_runs_csv(
    all_stats: &[Vec<rins::analysis::YearStats>],
    start_seed: u64,
    expense_ratio: f64,
    path: &str,
) {
    const CENTS_PER_BUSD: f64 = 100_000_000_000.0;
    let file = File::create(path).unwrap_or_else(|e| panic!("failed to create {path}: {e}"));
    let mut w = BufWriter::new(file);
    writeln!(w, "seed,year,loss_ratio,combined_ratio,rate_on_line,total_cap_b,cat_events,insolvent_count,dropped_count,entrant_count")
        .expect("write");
    for (i, run) in all_stats.iter().enumerate() {
        let seed = start_seed + i as u64;
        for s in run {
            writeln!(
                w,
                "{},{},{:.6},{:.6},{:.6},{:.6},{},{},{},{}",
                seed,
                s.year,
                s.loss_ratio(),
                s.combined_ratio(expense_ratio),
                s.rate_on_line(),
                s.total_capital as f64 / CENTS_PER_BUSD,
                s.cat_event_count,
                s.insolvent_count,
                s.dropped_count,
                s.entrant_count,
            )
            .expect("write");
        }
    }
}

fn print_all_run_years(
    all_stats: &[Vec<rins::analysis::YearStats>],
    start_seed: u64,
    expense_ratio: f64,
) {
    const CENTS_PER_BUSD: f64 = 100_000_000_000.0;

    println!("\n=== Per-Run Year Data ===");
    println!(
        "{:>6} | {:>4} | {:>7} | {:>7} | {:>6} | {:>11} | {:>5} | {:>6} | {:>5} | {:>5}",
        "Seed", "Year", "LossR%", "CombR%", "Rate%", "TotalCap(B)", "Cats#", "Insol#",
        "Drop#", "Entr#"
    );
    println!("{}", "-".repeat(80));

    for (i, run) in all_stats.iter().enumerate() {
        let seed = start_seed + i as u64;
        for s in run {
            println!(
                "{:>6} | {:>4} | {:>6.1}% | {:>6.1}% | {:>5.2}% | {:>11.2} | {:>5} | {:>6} | {:>5} | {:>5}",
                seed,
                s.year,
                s.loss_ratio() * 100.0,
                s.combined_ratio(expense_ratio) * 100.0,
                s.rate_on_line() * 100.0,
                s.total_capital as f64 / CENTS_PER_BUSD,
                s.cat_event_count,
                s.insolvent_count,
                s.dropped_count,
                s.entrant_count,
            );
        }
    }
}

fn print_dist_section<F>(title: &str, dists: &[rins::analysis::YearDist], scale: f64, extract: F)
where
    F: Fn(&rins::analysis::YearDist) -> &rins::analysis::DistStats,
{
    println!("\n--- {title} ---");
    println!(
        "{:>4} | {:>7} | {:>7} | {:>7} | {:>7} | {:>7} | {:>7} | {:>7} | {:>7} | {:>7}",
        "Year", "min", "p5", "p25", "p50", "p75", "p95", "max", "mean", "stddev"
    );
    for yd in dists {
        let ds = extract(yd);
        println!(
            "{:>4} | {:>7.1} | {:>7.1} | {:>7.1} | {:>7.1} | {:>7.1} | {:>7.1} | {:>7.1} | {:>7.1} | {:>7.1}",
            yd.year,
            ds.min * scale,
            ds.p5 * scale,
            ds.p25 * scale,
            ds.p50 * scale,
            ds.p75 * scale,
            ds.p95 * scale,
            ds.max * scale,
            ds.mean * scale,
            ds.std_dev * scale,
        );
    }
}

fn print_distributions(dists: &[rins::analysis::YearDist], n_runs: u64) {
    println!("\n=== Multi-Run Distribution (N={n_runs} runs) ===");

    print_dist_section("LossR%", dists, 100.0, |yd| &yd.loss_ratio);
    print_dist_section("Rate%", dists, 100.0, |yd| &yd.rate_on_line);
    print_dist_section("CombR%", dists, 100.0, |yd| &yd.combined_ratio);
    print_dist_section("TotalCap (B USD)", dists, 1.0, |yd| &yd.total_cap_b);

    println!("\n--- Discrete Counts (p50 | max) ---");
    println!(
        "{:>4} | {:>8} | {:>8} | {:>9} | {:>9} | {:>8} | {:>8} | {:>8} | {:>8}",
        "Year", "Cats p50", "Cats max", "Insol p50", "Insol max", "Drop p50", "Drop max",
        "Entr p50", "Entr max"
    );
    for yd in dists {
        println!(
            "{:>4} | {:>8} | {:>8} | {:>9} | {:>9} | {:>8} | {:>8} | {:>8} | {:>8}",
            yd.year,
            yd.cat_events.p50,
            yd.cat_events.max,
            yd.insolvents.p50,
            yd.insolvents.max,
            yd.dropped.p50,
            yd.dropped.max,
            yd.entrants.p50,
            yd.entrants.max,
        );
    }
}
