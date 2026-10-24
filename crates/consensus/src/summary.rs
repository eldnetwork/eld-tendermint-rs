//! Summary updates after a commit.
//!
//! [`crate::Node`] boot passes the execution app to [`SummaryManager::new`].
//! [`SummaryManager::update_summary`] builds one request for that frontier, sends
//! it, and logs the response.

use std::sync::Arc;

use eld_tendermint_proto::abci::{RequestQuery, ResponseQuery};
use eld_tendermint_types::{Level, log_line};

const FRONTIER_APPSTATE_PATH: &str = "/frontier_appstate";
const SNAPSHOT_QUERY_TOKEN_KEY: &str = "ELD_SNAPSHOT_QUERY_TOKEN";

/// Runs one ABCI query. The node crate implements this on `AbciApp`.
///
/// Consensus cannot call `AbciApp` directly: the node crate already depends on this one.
pub trait SummaryQuery: Send + Sync {
    /// # Errors
    ///
    /// Returns the socket error as text. The default reports that no query socket is connected.
    fn query(&self, _request: RequestQuery) -> Result<ResponseQuery, String> {
        Err("summary query is not connected".to_owned())
    }
}

/// Holds the summary update for one node.
pub struct SummaryManager {
    app: Arc<dyn SummaryQuery>,
}

impl SummaryManager {
    /// Creates the manager. Does not send a request.
    #[must_use]
    pub fn new(app: impl SummaryQuery + 'static) -> Self {
        Self { app: Arc::new(app) }
    }

    /// Asks the app for the summary at `frontier` and logs the response.
    pub fn update_summary(&self, frontier: i64) {
        let token = match std::env::var(SNAPSHOT_QUERY_TOKEN_KEY) {
            Ok(token) if !token.is_empty() => token,
            Ok(_) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "ELD_SNAPSHOT_QUERY_TOKEN is empty",
                    &[],
                );
                return;
            }
            Err(_) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "ELD_SNAPSHOT_QUERY_TOKEN is unset",
                    &[],
                );
                return;
            }
        };
        let mut data = token.into_bytes();
        data.extend_from_slice(&frontier.to_le_bytes());
        let request = RequestQuery {
            data: data.into(),
            path: FRONTIER_APPSTATE_PATH.to_owned(),
            height: 0,
            prove: false,
        };
        let height = frontier.to_string();
        match self.app.query(request) {
            Ok(response) => {
                let code = response.code.to_string();
                let bytes = response.value.len().to_string();
                log_line(
                    Level::Info,
                    "consensus",
                    "frontier app state response",
                    &[
                        ("frontier", height.as_str()),
                        ("code", code.as_str()),
                        ("bytes", bytes.as_str()),
                        ("log", response.log.as_str()),
                    ],
                );
            }
            Err(err) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "frontier app state query failed",
                    &[("frontier", height.as_str()), ("err", err.as_str())],
                );
            }
        }
    }
}
