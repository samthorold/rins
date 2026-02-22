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
            _ => {}
        }
        i += 1;
    }

    let mut config = SimulationConfig::canonical();
    if let Some(s) = seed_override {
        config.seed = s;
    }

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
