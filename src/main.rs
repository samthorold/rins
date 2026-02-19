mod broker;
mod events;
mod market;
mod simulation;
mod syndicate;
mod types;

fn main() {
    // Run for 5 simulated years (the natural production usage).
    let mut sim = simulation::Simulation::new(42).until(types::Day::year_end(types::Year(5)));
    sim.schedule(
        types::Day::year_start(types::Year(1)),
        events::Event::SimulationStart {
            year_start: types::Year(1),
        },
    );
    sim.run();
    println!("Events fired: {}", sim.log.len());
    for e in &sim.log {
        println!("  day={:5}  {:?}", e.day.0, e.event);
    }
}
