mod broker;
mod config;
mod events;
mod insured;
mod insurer;
mod market;
mod perils;
mod simulation;
mod types;

use std::fs::File;
use std::io::{BufWriter, Write};

use config::SimulationConfig;
use simulation::Simulation;

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

            // Report final insurer capitals.
            println!("\nFinal insurer capitals (end of simulation):");
            for ins in &sim.insurers {
                println!("  Insurer {:?}: {} cents", ins.id, ins.capital);
            }
        }
    }
}
