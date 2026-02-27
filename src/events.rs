use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId, Year};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Peril {
    WindstormAtlantic,
    Attritional,
}

/// The risk being submitted for coverage.
/// Full coverage: the insurer writes limit = sum_insured, attachment = 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    pub sum_insured: u64, // monetary units (e.g. USD cents)
    pub territory: String,
    pub perils_covered: Vec<Peril>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclineReason {
    MaxLineSizeExceeded,
    MaxCatAggregateBreached,
    Insolvent,
    /// Insurer is in voluntary runoff; no new business accepted until re-entry.
    InRunoff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// Fires once at Day(0) to bootstrap the simulation. Schedules YearStart(year_start).
    /// `warmup_years` warm-up years are prepended before the `analysis_years` analysis period;
    /// analysis scripts skip years ≤ warmup_years when generating output tables.
    SimulationStart { year_start: Year, warmup_years: u32, analysis_years: u32 },
    /// Fires at the start of each simulated year.
    YearStart { year: Year },
    /// Fires at the end of each simulated year.
    YearEnd { year: Year },
    /// An insured requests coverage for the year. Broker routes to a lead insurer.
    CoverageRequested { insured_id: InsuredId, risk: Risk },
    /// Broker asks the selected lead insurer to price a risk.
    LeadQuoteRequested {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        risk: Risk,
    },
    /// Lead insurer declined to quote — exposure limit breached.
    /// Broker will re-route to the next insurer.
    LeadQuoteDeclined {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        reason: DeclineReason,
    },
    /// Lead insurer has priced the risk and issued a quote.
    LeadQuoteIssued {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        atp: u64,                  // actuarial technical price (break-even floor)
        premium: u64,              // final quoted premium (underwriter decision)
        cat_exposure_at_quote: u64, // insurer's WindstormAtlantic aggregate before this risk is added (0 if risk doesn't cover cat)
    },
    /// Broker presents the quote to the insured.
    QuotePresented {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    },
    /// Insured accepts the quote. Market creates the policy record.
    QuoteAccepted {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    },
    /// Insured rejects the quote (rate on line exceeds max_rate_on_line).
    /// The simulation schedules a renewal CoverageRequested at the same annual offset.
    QuoteRejected { submission_id: SubmissionId, insured_id: InsuredId },
    /// All insurers declined this submission (capacity constraint or insolvency).
    /// The insured is uninsured for the year; the simulation schedules a retry at next renewal.
    SubmissionDropped { submission_id: SubmissionId, insured_id: InsuredId },
    /// Policy is formally bound. Activates the policy for loss routing.
    PolicyBound {
        policy_id: PolicyId,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
        sum_insured: u64, // makes the event self-contained for exposure analysis
        /// Insurer's total WindstormAtlantic aggregate after this policy is added.
        /// Zero for risks that do not cover WindstormAtlantic.
        total_cat_exposure: u64,
    },
    PolicyExpired {
        policy_id: PolicyId,
    },
    #[allow(clippy::enum_variant_names)] // LossEvent is a domain term, not a naming error
    LossEvent {
        event_id: u64,
        peril: Peril,
        /// Geographic territory struck by this event. Drawn uniformly from
        /// `CatConfig.territories` at scheduling time; `on_loss_event` filters
        /// `insured_registry` to only emit `AssetDamage` for matching insureds.
        territory: String,
    },
    /// A peril has damaged an insured's assets. Fired for every registered insured
    /// regardless of whether they hold an active policy. The market handler
    /// `on_asset_damage` routes to `ClaimSettled` only for covered insureds.
    AssetDamage { insured_id: InsuredId, peril: Peril, ground_up_loss: u64 },
    ClaimSettled {
        policy_id: PolicyId,
        insurer_id: InsurerId,
        amount: u64,
        peril: Peril,
        /// Insurer's capital remaining after this claim is paid (floored at zero).
        remaining_capital: u64,
    },
    /// Emitted the first time a claim drives an insurer's capital to zero.
    /// From this point on the insurer declines all new quote requests.
    InsurerInsolvent { insurer_id: InsurerId },
    /// A new insurer has entered the market, spawned by the coordinator after observing
    /// sustained market profitability. Logged at the YearEnd day that triggered entry.
    InsurerEntered {
        insurer_id: InsurerId,
        initial_capital: u64,
    },
    /// An insurer has voluntarily entered runoff after persistent above-threshold combined
    /// ratios. The insurer stops writing new business; in-force policies run to expiry.
    /// Logged directly (not dispatched) at the YearEnd that triggered the decision.
    InsurerExited { insurer_id: InsurerId },
    /// A runoff insurer has re-entered the market after the market AP/TP factor exceeded
    /// the entry threshold. Logged directly (not dispatched) at the triggering YearEnd.
    InsurerReEntered { insurer_id: InsurerId },
}

/// A dispatched event with its simulation day. Position in `Simulation.log` is its implicit sequence number.
///
/// Serves as both the immutable log entry and the priority queue entry. Ordering is by `day` only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimEvent {
    pub day: Day,
    pub event: Event,
}

impl Ord for SimEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.day.cmp(&other.day)
    }
}

impl PartialOrd for SimEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Append-only log of dispatched events.  `log[i]` has implicit sequence
/// number `i` (see `docs/event-sourcing.md §1`).
///
/// Mutation is restricted to `push`.  Use `from_history` to seed the log
/// from a pre-built slice (testing and checkpointing only).
#[derive(Debug, PartialEq)]
pub struct EventLog(Vec<SimEvent>);

impl EventLog {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Seed from an existing event list.  Intended for tests and
    /// future checkpoint replay — not for production simulation paths.
    pub fn from_history(events: Vec<SimEvent>) -> Self {
        Self(events)
    }

    pub fn push(&mut self, ev: SimEvent) {
        self.0.push(ev);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SimEvent> {
        self.0.iter()
    }

    /// Mutable reference to the most recently pushed entry.
    /// Used by dispatch handlers to back-fill computed fields (e.g. remaining_capital)
    /// into an event immediately after it is processed.
    pub fn last_mut(&mut self) -> Option<&mut SimEvent> {
        self.0.last_mut()
    }
}

impl std::ops::Deref for EventLog {
    type Target = [SimEvent];
    fn deref(&self) -> &[SimEvent] {
        &self.0
    }
}

impl<'a> IntoIterator for &'a EventLog {
    type Item = &'a SimEvent;
    type IntoIter = std::slice::Iter<'a, SimEvent>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufWriter, Write};

    use super::*;
    use crate::types::{InsurerId, SubmissionId};

    #[test]
    fn peril_covered_membership() {
        let risk = Risk {
            sum_insured: 1_000_000,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        assert!(risk.perils_covered.contains(&Peril::WindstormAtlantic));
        assert!(!risk.perils_covered.contains(&Peril::Attritional));
    }

    #[test]
    fn sim_event_serializes_day_and_event_fields() {
        let ev = SimEvent {
            day: Day(42),
            event: Event::YearEnd { year: Year(3) },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"day":42,"event":{"YearEnd":{"year":3}}}"#);
    }

    #[test]
    fn simulation_start_json_shape() {
        let ev = SimEvent {
            day: Day(0),
            event: Event::SimulationStart { year_start: Year(1), warmup_years: 0, analysis_years: 1 },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"day":0,"event":{"SimulationStart":{"year_start":1,"warmup_years":0,"analysis_years":1}}}"#);
    }

    #[test]
    fn policy_bound_serializes() {
        let ev = SimEvent {
            day: Day(10),
            event: Event::PolicyBound {
                policy_id: PolicyId(0),
                submission_id: SubmissionId(1),
                insured_id: InsuredId(5),
                insurer_id: InsurerId(2),
                premium: 50_000,
                sum_insured: 5_000_000_000,
                total_cat_exposure: 7_000_000_000,
            },
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert_eq!(value["event"]["PolicyBound"]["policy_id"], 0);
        assert_eq!(value["event"]["PolicyBound"]["insurer_id"], 2);
        assert_eq!(value["event"]["PolicyBound"]["insured_id"], 5);
        assert_eq!(value["event"]["PolicyBound"]["premium"], 50_000);
        assert_eq!(value["event"]["PolicyBound"]["sum_insured"], 5_000_000_000u64);
        assert_eq!(value["event"]["PolicyBound"]["total_cat_exposure"], 7_000_000_000u64);
    }

    #[test]
    fn ndjson_stream_one_line_per_event() {
        let events = vec![
            SimEvent {
                day: Day(0),
                event: Event::SimulationStart { year_start: Year(1), warmup_years: 0, analysis_years: 1 },
            },
            SimEvent {
                day: Day(359),
                event: Event::YearEnd { year: Year(1) },
            },
            SimEvent {
                day: Day(180),
                event: Event::LossEvent { event_id: 1, peril: Peril::WindstormAtlantic, territory: "US-SE".to_string() },
            },
        ];

        let mut buf: Vec<u8> = Vec::new();
        {
            let mut writer = BufWriter::new(&mut buf);
            for e in &events {
                serde_json::to_writer(&mut writer, e).unwrap();
                writeln!(writer).unwrap();
            }
        }

        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.split('\n').filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.get("day").is_some(), "missing 'day' key in: {line}");
            assert!(v.get("event").is_some(), "missing 'event' key in: {line}");
        }
    }

    #[test]
    fn quote_chain_events_serialize() {
        let ev = SimEvent {
            day: Day(1),
            event: Event::LeadQuoteRequested {
                submission_id: SubmissionId(0),
                insured_id: InsuredId(1),
                insurer_id: InsurerId(1),
                risk: Risk {
                    sum_insured: 1_000_000,
                    territory: "US-SE".to_string(),
                    perils_covered: vec![Peril::WindstormAtlantic],
                },
            },
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert!(value["event"]["LeadQuoteRequested"].is_object());
    }
}
