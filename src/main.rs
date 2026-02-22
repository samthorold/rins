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
use events::Event;
use simulation::Simulation;
use types::{Day, Year};

fn main() {
    let mut sim = Simulation::from_config(SimulationConfig::canonical());

    sim.schedule(Day(0), Event::SimulationStart { year_start: Year(1) });
    sim.run();

    let file = File::create("events.ndjson").expect("failed to create events.ndjson");
    let mut writer = BufWriter::new(file);
    for e in &sim.log {
        serde_json::to_writer(&mut writer, e).expect("failed to serialize event");
        writeln!(writer).expect("failed to write newline");
    }

    println!("Events fired: {}", sim.log.len());

    // Report final insurer capitals.
    println!("\nFinal insurer capitals (end of simulation):");
    for ins in &sim.insurers {
        println!("  Insurer {:?}: {} cents", ins.id, ins.capital);
    }
}
