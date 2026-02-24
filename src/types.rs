use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InsuredId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SubmissionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InsurerId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PolicyId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Year(pub u32);

/// Simulation time in days (1 unit = 1 simulated day).
/// Uses the insurance convention of 360 days per year (12 × 30-day months).
/// Time jumps directly from one event to the next — there is no clock
/// ticking through the gaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Day(pub u64);

impl Day {
    pub const DAYS_PER_YEAR: u64 = 360;

    pub fn year_start(year: Year) -> Self {
        Day((year.0 as u64 - 1) * Self::DAYS_PER_YEAR)
    }

    pub fn year_end(year: Year) -> Self {
        Day(year.0 as u64 * Self::DAYS_PER_YEAR - 1)
    }

    pub fn offset(self, days: u64) -> Self {
        Day(self.0 + days)
    }

    /// Convert this day to the simulation year it falls in (1-indexed).
    pub fn year(self) -> Year {
        Year((self.0 / Self::DAYS_PER_YEAR) as u32 + 1)
    }
}

/// Mutable per-year accumulator for premium and claims.
/// Held by agents to track year-to-date financials; reset at each YearEnd.
#[derive(Debug, Default, Clone)]
pub struct YearAccumulator {
    /// Gross premium written (cents).
    pub premium: u64,
    /// Total claims paid, all perils (cents).
    pub total_claims: u64,
    /// Attritional claims paid (cents).
    pub attritional_claims: u64,
    /// Sum insured written (cents). Used as EWMA denominator.
    pub exposure: u64,
}

impl YearAccumulator {
    /// Reset all counters to zero (call at YearEnd after deriving metrics).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Loss ratio: total_claims / premium. Returns 0.0 if no premium written.
    pub fn loss_ratio(&self) -> f64 {
        if self.premium == 0 { 0.0 } else { self.total_claims as f64 / self.premium as f64 }
    }

    /// Attritional loss fraction: attritional_claims / exposure. Returns 0.0 if no exposure.
    pub fn attritional_loss_fraction(&self) -> f64 {
        if self.exposure == 0 {
            0.0
        } else {
            self.attritional_claims as f64 / self.exposure as f64
        }
    }
}
