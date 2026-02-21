use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct InsuredId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SubmissionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SyndicateId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct BrokerId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct PolicyId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct LossEventId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Year(pub u32);

/// Simulation time in days (1 unit = 1 simulated day).
/// Uses the insurance convention of 360 days per year (12 × 30-day months).
/// Time jumps directly from one event to the next — there is no clock
/// ticking through the gaps. Use explicit offsets when events within the
/// same process need a defined relative order (e.g. lead quote on day D,
/// follower quotes on day D+1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Day(pub u64);

impl Day {
    pub const DAYS_PER_YEAR: u64 = 360;

    pub fn year_start(year: Year) -> Self {
        Day((year.0 as u64 - 1) * Self::DAYS_PER_YEAR)
    }

    pub fn year_end(year: Year) -> Self {
        Day(year.0 as u64 * Self::DAYS_PER_YEAR - 1)
    }

    /// Advance by a number of days — used to sequence events within a round
    /// without requiring agents to coordinate scheduling order.
    pub fn offset(self, days: u64) -> Self {
        Day(self.0 + days)
    }
}
