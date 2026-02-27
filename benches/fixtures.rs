use rins::config::{AttritionalConfig, CatConfig, InsurerConfig, SimulationConfig};
use rins::events::{Event, Peril, Risk};
use rins::market::Market;
use rins::simulation::Simulation;
use rins::types::{Day, InsuredId, InsurerId, SubmissionId, Year};

pub struct Scenario {
    pub n_insureds: usize,
    pub insurer_count: usize,
}

pub const SMALL: Scenario = Scenario { n_insureds: 10, insurer_count: 3 };

pub const MEDIUM: Scenario = Scenario { n_insureds: 100, insurer_count: 5 };

pub const LARGE: Scenario = Scenario { n_insureds: 1000, insurer_count: 10 };

fn default_risk() -> Risk {
    Risk {
        sum_insured: 5_000_000_000,
        territory: "US-SE".to_string(),
        perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
    }
}

/// Bind `policy_count` policies directly into `market` using the public API.
pub fn prepopulate_policies(market: &mut Market, policy_count: usize) {
    for i in 0..policy_count {
        let sid = SubmissionId(i as u64);
        let iid = InsuredId(i as u64 + 1);
        let events = market.on_quote_accepted(
            Day(0),
            sid,
            iid,
            InsurerId(1),
            100_000,
            default_risk(),
            Year(1),
        );
        // Activate the policy (simulate PolicyBound firing).
        for (_, e) in &events {
            if let Event::PolicyBound { policy_id, .. } = e {
                market.on_policy_bound(*policy_id);
            }
        }
    }
}

/// Build a full `Simulation` ready to run for `years`.
pub fn build_simulation(scenario: &Scenario, seed: u64, years: u32) -> Simulation {
    let config = SimulationConfig {
        seed,
        years,
        warmup_years: 0,
        insurers: (1..=scenario.insurer_count as u64)
            .map(|i| InsurerConfig {
                id: InsurerId(i),
                initial_capital: 100_000_000_000,
                attritional_elf: 0.239,
                cat_elf: 0.0,
                target_loss_ratio: 0.70,
                ewma_credibility: 0.3,
                expense_ratio: 0.344,
                profit_loading: 0.0,
                net_line_capacity: Some(0.30),
                solvency_capital_fraction: Some(0.30),
                pml_damage_fraction_override: None,
                depletion_sensitivity: 0.0,
            })
            .collect(),
        n_insureds: scenario.n_insureds,
        attritional: AttritionalConfig { annual_rate: 2.0, mu: -3.0, sigma: 1.0 },
        catastrophe: CatConfig { annual_frequency: 0.5, pareto_scale: 0.05, pareto_shape: 1.5, max_damage_fraction: 1.0, territories: vec!["US-SE".to_string()] },
        quotes_per_submission: None,
        max_rate_on_line: 1.0,
        disable_cats: false,
        runoff_cr_threshold: 2.0,
        capital_exit_floor: 0.0,
    };
    let mut sim = Simulation::from_config(config);
    sim.start();
    sim
}
