//! Summary updates after a commit.
//!
//! [`SummaryManager`] is created in [`crate::Node`] boot. [`SummaryManager::update_summary`]
//! will ask the app for the summary at one frontier. The query is not wired yet.

const FRONTIER_APPSTATE_PATH: &str = "/frontier_appstate";

/// Holds the summary update for one node.
pub struct SummaryManager;

impl SummaryManager {
    /// Creates the manager. Does not send a request.
    #[must_use]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }

    /// Updates the summary for `frontier`.
    pub fn update_summary(&self, frontier: i64) {
        let _ = (self, frontier, FRONTIER_APPSTATE_PATH);
    }
}
