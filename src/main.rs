mod broker;
mod events;
mod market;
mod perils;
mod simulation;
mod syndicate;
mod types;

use std::fs::File;
use std::io::{BufWriter, Write};

use broker::Broker;
use events::{Peril, Risk};
use simulation::Simulation;
use syndicate::Syndicate;
use types::{BrokerId, Day, SyndicateId, Year};

fn main() {
    let risk = Risk {
        line_of_business: "property".to_string(),
        sum_insured: 2_000_000,
        territory: "US-SE".to_string(),
        limit: 1_000_000,
        attachment: 100_000,
        perils_covered: vec![Peril::WindstormAtlantic, Peril::Flood],
    };

    let syndicates = vec![
        Syndicate::new(SyndicateId(1), 50_000_000, 500),
        Syndicate::new(SyndicateId(2), 40_000_000, 600),
        Syndicate::new(SyndicateId(3), 30_000_000, 450),
    ];

    let brokers = vec![
        Broker::new(BrokerId(1), 3, vec![risk.clone()]),
        Broker::new(BrokerId(2), 2, vec![risk]),
    ];

    let mut sim = Simulation::new(42)
        .until(Day::year_end(Year(5)))
        .with_agents(syndicates, brokers);

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
