use crate::syndicate::Syndicate;
use crate::types::Year;

/// Industry-wide statistics published at year-end.
/// Syndicates read these when pricing for the next year.
pub struct YearStats {
    pub year: Year,
    pub industry_loss_ratio: f64, // placeholder; will derive from events
    pub active_syndicate_count: usize,
}

pub struct Market;

impl Market {
    pub fn new() -> Self {
        Market
    }

    /// Compute industry statistics from the active syndicate pool.
    /// Returns an owned value — caller can then mutably borrow agents.
    pub fn compute_year_stats(&self, syndicates: &[Syndicate], year: Year) -> YearStats {
        YearStats {
            year,
            industry_loss_ratio: 0.0, // TODO: derive from event log
            active_syndicate_count: syndicates.len(),
        }
    }
}
