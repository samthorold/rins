use std::cmp::Ordering;

use crate::types::{BrokerId, Day, LossEventId, PolicyId, SubmissionId, SyndicateId, Year};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Risk {
    pub line_of_business: String,
    pub sum_insured: u64, // pence
    pub territory: String,
    pub limit: u64,
    pub attachment: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelEntry {
    pub syndicate_id: SyndicateId,
    pub share_bps: u32, // basis points; entries must sum to 10_000
    pub premium: u64,   // syndicate's share, pence
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub entries: Vec<PanelEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // LossEvent is a domain term, not a naming error
pub enum Event {
    SimulationStart {
        year_start: Year,
    },
    YearEnd {
        year: Year,
    },
    SubmissionArrived {
        submission_id: SubmissionId,
        broker_id: BrokerId,
        risk: Risk,
    },
    QuoteRequested {
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
        is_lead: bool,
    },
    QuoteIssued {
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
        premium: u64,
        is_lead: bool,
    },
    QuoteDeclined {
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
    },
    PolicyBound {
        submission_id: SubmissionId,
        panel: Panel,
    },
    LossEvent {
        event_id: LossEventId,
        region: String,
        peril: String,
        severity: u64,
    },
    ClaimSettled {
        policy_id: PolicyId,
        syndicate_id: SyndicateId,
        amount: u64,
    },
    SyndicateEntered {
        syndicate_id: SyndicateId,
    },
    SyndicateInsolvency {
        syndicate_id: SyndicateId,
    },
}

/// Unified event record — serves as both the immutable log entry and the
/// priority queue entry. Ordering is by `day` only; `Event` has no
/// meaningful ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
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
