mod broker;
mod config;
mod events;
mod market;
mod perils;
mod simulation;
mod syndicate;
mod types;

use std::fs::File;
use std::io::{BufWriter, Write};

use config::SimulationConfig;
use simulation::Simulation;
use types::{Day, Year};

fn main() {
    let mut sim = Simulation::from_config(&SimulationConfig::canonical());

    sim.schedule(
        Day::year_start(Year(1)),
        events::Event::SimulationStart {
            year_start: Year(1),
        },
    );
    sim.run();

    let file = File::create("events.ndjson").expect("failed to create events.ndjson");
    let mut writer = BufWriter::new(file);
    for e in &sim.log {
        serde_json::to_writer(&mut writer, e).expect("failed to serialize event");
        writeln!(writer).expect("failed to write newline");
    }

    println!("Events fired: {}", sim.log.len());
    for e in &sim.log {
        println!("  day={:5}  {:?}", e.day.0, e.event);
    }

    // Report final syndicate capitals.
    println!("\nFinal syndicate capitals:");
    for s in &sim.syndicates {
        println!("  Syndicate {:?}: {} pence", s.id, s.capital);
    }
}
