//! Signed capacity-proof summary a proposer may put in a block.
//!
//! The bytes come from the app. They are not a mempool transaction.

/// Read the summary for the epoch of a proposal height.
pub trait CapacitySummary: Send + Sync {
    /// JSON bytes of the summary for the epoch of `height`.
    ///
    /// `None` when that epoch has no signed summary.
    fn capacity_proof_summary(&self, _height: i64) -> Option<Vec<u8>> {
        None
    }
}
