use std::collections::HashMap;
use std::env;

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use rins::config::SimulationConfig;
use rins::perils::generate_cat_catalog;

fn main() {
    let config = SimulationConfig::canonical();

    let n_years: u32 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(config.years);

    let mut rng = ChaCha20Rng::seed_from_u64(config.seed);
    let entries = generate_cat_catalog(&config.catastrophe, n_years, &mut rng);

    // Write NDJSON to stdout.
    for entry in &entries {
        println!("{}", serde_json::to_string(entry).expect("serialisation failed"));
    }

    // Per-territory summary to stderr.
    let mut territory_counts: HashMap<&str, usize> = HashMap::new();
    let mut territory_sum_df: HashMap<&str, f64> = HashMap::new();
    let mut territory_max_df: HashMap<&str, f64> = HashMap::new();
    // Per-class summary.
    let mut class_counts: HashMap<&str, usize> = HashMap::new();
    let mut class_sum_df: HashMap<&str, f64> = HashMap::new();
    for e in &entries {
        let t = e.territory.as_str();
        *territory_counts.entry(t).or_insert(0) += 1;
        *territory_sum_df.entry(t).or_insert(0.0) += e.damage_fraction;
        let cur = territory_max_df.entry(t).or_insert(0.0);
        if e.damage_fraction > *cur {
            *cur = e.damage_fraction;
        }
        let c = e.class.as_str();
        *class_counts.entry(c).or_insert(0) += 1;
        *class_sum_df.entry(c).or_insert(0.0) += e.damage_fraction;
    }

    let total_annual_frequency: f64 = config.catastrophe.event_classes.iter().map(|c| c.annual_frequency).sum();
    eprintln!(
        "cat_catalog: {} years, {} total events (expected ~{:.1})",
        n_years,
        entries.len(),
        total_annual_frequency * n_years as f64
    );

    // Class breakdown.
    let mut classes: Vec<&str> = class_counts.keys().copied().collect();
    classes.sort_unstable();
    for c in classes {
        let n = class_counts[c];
        let mean = class_sum_df[c] / n as f64;
        eprintln!("  class={c:<8}  events={n:>4}  mean_df={mean:.4}");
    }

    // Territory breakdown.
    let mut territories: Vec<&str> = territory_counts.keys().copied().collect();
    territories.sort_unstable();
    for t in territories {
        let n = territory_counts[t];
        let mean = territory_sum_df[t] / n as f64;
        let max = territory_max_df[t];
        eprintln!("  territory={t:<12}  events={n:>4}  mean_df={mean:.4}  max_df={max:.4}");
    }
}
