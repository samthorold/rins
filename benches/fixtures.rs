use rins::broker::Broker;
use rins::events::{Event, Panel, PanelEntry, Peril, Risk};
use rins::market::Market;
use rins::simulation::Simulation;
use rins::syndicate::Syndicate;
use rins::types::{BrokerId, Day, SubmissionId, SyndicateId, Year};

pub struct Scenario {
    pub syndicates: usize,
    pub brokers: usize,
    pub submissions_per_broker: usize,
    pub initial_capital: u64,
    pub rate_on_line_bps: u32,
}

pub const SMALL: Scenario = Scenario {
    syndicates: 5,
    brokers: 2,
    submissions_per_broker: 10,
    initial_capital: 50_000_000,
    rate_on_line_bps: 500,
};

pub const MEDIUM: Scenario = Scenario {
    syndicates: 20,
    brokers: 10,
    submissions_per_broker: 100,
    initial_capital: 50_000_000,
    rate_on_line_bps: 500,
};

pub const LARGE: Scenario = Scenario {
    syndicates: 80,
    brokers: 25,
    submissions_per_broker: 500,
    initial_capital: 50_000_000,
    rate_on_line_bps: 500,
};

pub fn make_syndicates(n: usize, capital: u64, rate: u32) -> Vec<Syndicate> {
    (1..=n)
        .map(|i| Syndicate::new(SyndicateId(i as u64), capital, rate))
        .collect()
}

pub fn make_brokers(n: usize, subs_per_broker: usize) -> Vec<Broker> {
    let territories = ["US-SE", "UK", "JP"];
    let perils: [&[Peril]; 3] = [
        &[Peril::WindstormAtlantic],
        &[Peril::WindstormEuropean],
        &[Peril::EarthquakeJapan],
    ];
    (1..=n)
        .map(|i| {
            let idx = (i - 1) % 3;
            let risk = Risk {
                line_of_business: "property".to_string(),
                sum_insured: 2_000_000,
                territory: territories[idx].to_string(),
                limit: 1_000_000,
                attachment: 100_000,
                perils_covered: perils[idx].to_vec(),
            };
            Broker::new(BrokerId(i as u64), subs_per_broker, vec![risk])
        })
        .collect()
}

/// Bind `policy_count` policies into `market` via `on_policy_bound`.
/// All policies use territory "US-SE" and `Peril::WindstormAtlantic` so a single
/// `LossEvent` hits all of them. Shares are allocated equally with any remainder
/// assigned to entry 0.
pub fn prepopulate_policies(market: &mut Market, policy_count: usize, panel_size: usize) {
    let share_per = 10_000u32 / panel_size as u32;
    let remainder = 10_000u32 - share_per * panel_size as u32;
    for i in 0..policy_count {
        let entries: Vec<PanelEntry> = (0..panel_size)
            .map(|j| PanelEntry {
                syndicate_id: SyndicateId((j + 1) as u64),
                share_bps: if j == 0 { share_per + remainder } else { share_per },
                premium: 0,
            })
            .collect();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit: 5_000_000,
            attachment: 500_000,
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        market.on_policy_bound(SubmissionId(i as u64), risk, Panel { entries });
    }
}

/// Build a full `Simulation` ready to run, scheduled up to the end of `years`.
pub fn build_simulation(scenario: &Scenario, seed: u64, years: u32) -> Simulation {
    let syndicates =
        make_syndicates(scenario.syndicates, scenario.initial_capital, scenario.rate_on_line_bps);
    let brokers = make_brokers(scenario.brokers, scenario.submissions_per_broker);
    let mut sim = Simulation::new(seed)
        .until(Day::year_end(Year(years)))
        .with_agents(syndicates, brokers);
    sim.schedule(
        Day::year_start(Year(1)),
        Event::SimulationStart { year_start: Year(1) },
    );
    sim
}
