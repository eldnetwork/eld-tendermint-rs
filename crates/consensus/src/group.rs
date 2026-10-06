//! N validators in one process. Messages are method calls. Time is virtual.

use crate::round::{Msg, Node, Scheduled, Step};

/// In-process validators. `stop_at` ends [`Self::run_until_height`] once every
/// block store has reached that height, before a later height is committed.
pub struct Group<E, C>
where
    E: eld_tendermint_state::App,
    C: eld_tendermint_mempool::App,
{
    nodes: Vec<Node<E, C>>,
}

impl<E, C> Group<E, C>
where
    E: eld_tendermint_state::App,
    C: eld_tendermint_mempool::App,
{
    #[must_use]
    pub fn new(nodes: Vec<Node<E, C>>) -> Self {
        Self { nodes }
    }

    pub fn nodes(&mut self) -> &mut [Node<E, C>] {
        &mut self.nodes
    }

    /// Validators, in construction order.
    #[must_use]
    pub fn into_nodes(self) -> Vec<Node<E, C>> {
        self.nodes
    }

    /// Deliver queued messages, including each message back to its sender.
    /// `allow_to_others` drops a message for every validator except the sender.
    ///
    /// Returns when the outboxes are empty or every store height is at least `stop_at`.
    pub fn pump(&mut self, stop_at: i64, allow_to_others: impl Fn(&Msg) -> bool) {
        for _ in 0..10_000 {
            if self.all_at_least(stop_at) {
                return;
            }
            if !self.pump_once(stop_at, &allow_to_others) {
                return;
            }
        }
    }

    /// [`Self::pump`] everything, then fire the soonest timeout, until `height` is stored.
    ///
    /// The next height's round is started so its proposal exists. Those votes stay
    /// queued, so a later height is not committed.
    ///
    /// # Panics
    ///
    /// Panics when `height` is still missing after 10_000 rounds of pumping and timeouts.
    pub fn run_until_height(&mut self, height: i64) {
        for _ in 0..10_000 {
            if self.all_at_least(height) {
                self.start_next_height();
                return;
            }
            if !self.pump_once(height, &|_| true) {
                self.fire_timeouts();
            }
        }
        panic!("consensus did not commit height {height}");
    }

    #[must_use]
    pub fn all_at_least(&self, height: i64) -> bool {
        !self.nodes.is_empty() && self.nodes.iter().all(|node| node.store_height() >= height)
    }

    fn pump_once(&mut self, stop_at: i64, allow_to_others: &impl Fn(&Msg) -> bool) -> bool {
        let mut progress = false;
        for index in 0..self.nodes.len() {
            if self.all_at_least(stop_at) {
                return progress;
            }
            let msgs = self.nodes[index].take_outbox();
            if msgs.is_empty() {
                continue;
            }
            progress = true;
            for msg in msgs {
                self.nodes[index].deliver(msg.clone());
                if !allow_to_others(&msg) {
                    continue;
                }
                for other in 0..self.nodes.len() {
                    if other == index {
                        continue;
                    }
                    self.nodes[other].deliver(msg.clone());
                }
            }
        }
        progress
    }

    /// Deliver messages that match `pred`. Others stay queued.
    ///
    /// `only_other` delivers a message to that validator in addition to the sender.
    /// `None` delivers it to every validator.
    pub fn exchange(&mut self, only_other: Option<usize>, pred: impl Fn(&Msg) -> bool) {
        for _ in 0..10_000 {
            let mut progress = false;
            for index in 0..self.nodes.len() {
                let msgs = self.nodes[index].take_outbox();
                let mut skipped = Vec::new();
                for msg in msgs {
                    if !pred(&msg) {
                        skipped.push(msg);
                        continue;
                    }
                    progress = true;
                    self.nodes[index].deliver(msg.clone());
                    match only_other {
                        Some(dest) if dest != index => {
                            self.nodes[dest].deliver(msg);
                        }
                        Some(_) => {}
                        None => {
                            for other in 0..self.nodes.len() {
                                if other != index {
                                    self.nodes[other].deliver(msg.clone());
                                }
                            }
                        }
                    }
                }
                self.nodes[index].return_outbox(skipped);
            }
            if !progress {
                return;
            }
        }
    }

    /// Fire the timeout on one validator and leave the others waiting.
    pub fn fire_timeout(&mut self, index: usize) {
        let Some(timeout) = self.nodes[index].timeout.clone() else {
            panic!("validator {index} has no timeout");
        };
        self.nodes[index].timeout = None;
        self.nodes[index].on_timeout(&timeout);
    }

    fn start_next_height(&mut self) {
        for node in &mut self.nodes {
            let Some(timeout) = node.timeout.clone() else {
                continue;
            };
            if timeout.step == Step::NewHeight {
                node.timeout = None;
                node.on_timeout(&timeout);
            }
        }
    }

    fn fire_timeouts(&mut self) {
        let Some(delay) = self
            .nodes
            .iter()
            .filter_map(|node| node.timeout.as_ref().map(|timeout| timeout.delay_nanos))
            .min()
        else {
            panic!("consensus is idle with no timeout scheduled");
        };
        for node in &mut self.nodes {
            let Some(timeout) = node.timeout.clone() else {
                continue;
            };
            if timeout.delay_nanos == delay {
                node.timeout = None;
                node.on_timeout(&timeout);
            } else {
                node.timeout = Some(Scheduled {
                    delay_nanos: timeout.delay_nanos - delay,
                    ..timeout
                });
            }
        }
    }
}
