use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use crate::broker::Broker;
use crate::events::{Event, SimEvent};
use crate::market::Market;
use crate::syndicate::Syndicate;
use crate::types::{Day, Year};

pub struct Simulation {
    queue: BinaryHeap<Reverse<SimEvent>>,
    pub log: Vec<SimEvent>,
    rng: ChaCha20Rng,
    max_day: Option<Day>,
    max_events: Option<usize>,
    syndicates: Vec<Syndicate>,
    brokers: Vec<Broker>,
    market: Market,
}

impl Simulation {
    pub fn new(seed: u64) -> Self {
        Simulation {
            queue: BinaryHeap::new(),
            log: Vec::new(),
            rng: ChaCha20Rng::seed_from_u64(seed),
            max_day: None,
            max_events: None,
            syndicates: Vec::new(),
            brokers: Vec::new(),
            market: Market::new(),
        }
    }

    /// Builder: stop after this day (events scheduled past the horizon are
    /// never fired).
    pub fn until(mut self, day: Day) -> Self {
        self.max_day = Some(day);
        self
    }

    /// Builder: stop after N events fire (unit-test safety valve).
    pub fn with_max_events(mut self, n: usize) -> Self {
        self.max_events = Some(n);
        self
    }

    /// Builder: seed the agent pools.
    pub fn with_agents(mut self, syndicates: Vec<Syndicate>, brokers: Vec<Broker>) -> Self {
        self.syndicates = syndicates;
        self.brokers = brokers;
        self
    }

    /// Schedule an event to fire at the given day.
    pub fn schedule(&mut self, day: Day, event: Event) {
        self.queue.push(Reverse(SimEvent { day, event }));
    }

    /// Run the simulation until a stopping condition is met.
    pub fn run(&mut self) {
        let mut count = 0;
        loop {
            if let Some(max) = self.max_events
                && count >= max
            {
                break;
            }

            let next_day = match self.queue.peek() {
                Some(Reverse(ev)) => ev.day,
                None => break,
            };

            if let Some(horizon) = self.max_day
                && next_day > horizon
            {
                break;
            }

            let Reverse(ev) = self.queue.pop().unwrap();
            // Log cause before dispatching effect.
            self.log.push(ev.clone());
            self.dispatch(ev.day, ev.event);
            count += 1;
        }
    }

    fn dispatch(&mut self, day: Day, event: Event) {
        match event {
            Event::SimulationStart { year_start } => {
                self.handle_simulation_start(day, year_start);
            }
            Event::YearEnd { year } => {
                // 1. Coordinator derives industry stats (immutable read of agents).
                //    compute_year_stats returns owned YearStats, releasing the
                //    borrow before agents are mutated below.
                let _stats = self.market.compute_year_stats(&self.syndicates, year);

                // 2. Each Syndicate updates its actuarial state for next year's pricing.
                for s in &mut self.syndicates {
                    s.on_year_end(year, &mut self.rng);
                }

                // 3. Each Broker applies relationship decay.
                for b in &mut self.brokers {
                    b.on_year_end(year);
                }

                // 4. Schedule next year (keeps the sim running until max_day).
                self.handle_year_end(day, year);
            }
            Event::SubmissionArrived {
                submission_id,
                broker_id,
                risk,
            } => {
                let _ = (submission_id, broker_id, risk);
            }
            Event::QuoteRequested {
                submission_id,
                syndicate_id,
                is_lead,
            } => {
                let _ = (submission_id, syndicate_id, is_lead);
            }
            Event::QuoteIssued {
                submission_id,
                syndicate_id,
                premium,
                is_lead,
            } => {
                let _ = (submission_id, syndicate_id, premium, is_lead);
            }
            Event::QuoteDeclined {
                submission_id,
                syndicate_id,
            } => {
                let _ = (submission_id, syndicate_id);
            }
            Event::PolicyBound {
                submission_id,
                panel,
            } => {
                let _ = (submission_id, panel);
            }
            Event::LossEvent {
                event_id,
                region,
                peril,
                severity,
            } => {
                let _ = (event_id, region, peril, severity);
            }
            Event::ClaimSettled {
                policy_id,
                syndicate_id,
                amount,
            } => {
                let _ = (policy_id, syndicate_id, amount);
            }
            Event::SyndicateEntered { syndicate_id } => {
                let _ = syndicate_id;
            }
            Event::SyndicateInsolvency { syndicate_id } => {
                let _ = syndicate_id;
            }
        }
    }

    fn handle_simulation_start(&mut self, _day: Day, year_start: Year) {
        self.schedule(
            Day::year_end(year_start),
            Event::YearEnd { year: year_start },
        );
    }

    fn handle_year_end(&mut self, _day: Day, year: Year) {
        let next = Year(year.0 + 1);
        self.schedule(
            Day::year_start(next),
            Event::SimulationStart { year_start: next },
        );
        // YearEnd for `next` is scheduled by handle_simulation_start when
        // SimulationStart(next) fires — not here, to avoid double-scheduling.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use crate::types::{Day, Year};

    #[test]
    fn simulation_start_schedules_year_end() {
        // max_events=3 → fires: SimulationStart(1), YearEnd(1), SimulationStart(2)
        let mut sim = Simulation::new(0).with_max_events(3);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();
        let starts: Vec<u32> = sim
            .log
            .iter()
            .filter_map(|e| match &e.event {
                Event::SimulationStart { year_start } => Some(year_start.0),
                _ => None,
            })
            .collect();
        let ends: Vec<u32> = sim
            .log
            .iter()
            .filter_map(|e| match &e.event {
                Event::YearEnd { year } => Some(year.0),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![1, 2]);
        assert_eq!(ends, vec![1]);
    }

    #[test]
    fn year_end_fires_at_correct_day() {
        let mut sim = Simulation::new(0).with_max_events(2);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();
        let ye = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::YearEnd { .. }))
            .unwrap();
        assert_eq!(ye.day, Day::year_end(Year(1)));
    }

    #[test]
    fn log_is_day_ordered() {
        // Core DES invariant: log must be non-decreasing in day.
        let mut sim = Simulation::new(0).with_max_events(10);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();
        let days: Vec<u64> = sim.log.iter().map(|e| e.day.0).collect();
        let mut sorted = days.clone();
        sorted.sort_unstable();
        assert_eq!(days, sorted);
    }

    #[test]
    fn same_seed_produces_identical_logs() {
        let run = |seed: u64| {
            let mut sim = Simulation::new(seed).with_max_events(10);
            sim.schedule(
                Day::year_start(Year(1)),
                Event::SimulationStart {
                    year_start: Year(1),
                },
            );
            sim.run();
            sim.log
        };
        assert_eq!(run(42), run(42));
    }
}
